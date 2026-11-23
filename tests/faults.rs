//! Fault injection (`/_fakes3/faults`, `Server::add_fault`), checked with the real `aws-sdk-s3`
//! client: its retries, timeouts and error handling are what the feature exists to exercise.

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use aws_sdk_s3::config::retry::RetryConfig;
use aws_sdk_s3::config::timeout::TimeoutConfig;
use aws_sdk_s3::config::{BehaviorVersion, Credentials, Region};
use aws_sdk_s3::error::ProvideErrorMetadata;
use aws_sdk_s3::primitives::ByteStream;
use bytes::Bytes;
use fakes3::{Config, Delay, FaultKind, FaultRule, RequestFilter, ResetOptions, Server};
use http_body_util::{BodyExt, Full};
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use tokio::net::TcpStream;

mod common;

async fn start() -> Server {
    Server::start(Config::default()).await.unwrap()
}

/// A client that retries up to `attempts` times without waiting long between attempts.
fn client_with(s: &Server, attempts: u32, timeouts: Option<TimeoutConfig>) -> aws_sdk_s3::Client {
    let mut b = aws_sdk_s3::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .endpoint_url(s.endpoint())
        .region(Region::new("us-east-1"))
        .credentials_provider(Credentials::new("fakes3", "fakes3secret", None, None, "test"))
        .force_path_style(true)
        .retry_config(
            RetryConfig::standard()
                .with_max_attempts(attempts)
                .with_initial_backoff(Duration::from_millis(1))
                .with_max_backoff(Duration::from_millis(5)),
        );
    if let Some(t) = timeouts {
        b = b.timeout_config(t);
    }
    aws_sdk_s3::Client::from_conf(b.build())
}

fn client(s: &Server) -> aws_sdk_s3::Client {
    client_with(s, 3, None)
}

/// A client that does not retry, to see each fault as it is.
fn no_retry(s: &Server) -> aws_sdk_s3::Client {
    client_with(s, 1, None)
}

async fn seed(s3: &aws_sdk_s3::Client, bucket: &str, keys: &[&str]) {
    s3.create_bucket().bucket(bucket).send().await.unwrap();
    for k in keys {
        s3.put_object()
            .bucket(bucket)
            .key(*k)
            .body(ByteStream::from_static(b"hello"))
            .send()
            .await
            .unwrap();
    }
}

fn log(s: &Server, op: &str) -> Vec<fakes3::RequestRecord> {
    s.requests(&RequestFilter {
        operation: Some(op.into()),
        ..Default::default()
    })
    .0
}

#[tokio::test]
async fn slow_down_once_then_the_sdk_retry_succeeds() {
    let s = start().await;
    let s3 = client(&s);
    seed(&s3, "bkt", &[]).await;
    let rule = s.add_fault(FaultRule::slow_down().operation("PutObject").times(1)).unwrap();

    s3.put_object()
        .bucket("bkt")
        .key("k")
        .body(ByteStream::from_static(b"data"))
        .send()
        .await
        .unwrap();

    let puts = log(&s, "PutObject");
    assert_eq!(puts.iter().map(|r| r.status).collect::<Vec<_>>(), [503, 200]);
    let f = puts[0].fault.expect("first attempt is marked as injected");
    assert_eq!((f.id, f.kind, f.delayed), (rule.id, FaultKind::Error, false));
    assert!(puts[1].fault.is_none());
    let info = &s.faults()[0];
    assert_eq!((info.matched, info.fired, info.remaining()), (2, 1, Some(0)));
    let got = s3.get_object().bucket("bkt").key("k").send().await.unwrap();
    assert_eq!(got.body.collect().await.unwrap().into_bytes().as_ref(), b"data");
}

#[tokio::test]
async fn internal_error_is_an_s3_error_with_request_id() {
    let s = start().await;
    let s3 = client(&s);
    seed(&s3, "bkt", &["k"]).await;
    s.add_fault(FaultRule::internal_error().operation("GetObject")).unwrap();

    let err = s3.get_object().bucket("bkt").key("k").send().await.unwrap_err();
    let svc = err.into_service_error();
    assert_eq!(svc.code(), Some("InternalError"));
    assert_eq!(svc.message(), Some("We encountered an internal error. Please try again."));
    // Three attempts, all hit; each answer is a well-formed S3 error.
    let gets = log(&s, "GetObject");
    assert_eq!(gets.iter().map(|r| r.status).collect::<Vec<_>>(), [500, 500, 500]);

    let r = common::send(s.addr(), common::req("GET", "/bkt/k")).await;
    assert_eq!((r.status, r.code()), (500, "InternalError"));
    let id = r.header("x-amz-request-id").unwrap().to_owned();
    assert!(r.body.contains(&format!("<RequestId>{id}</RequestId>")), "{}", r.body);
    assert!(r.body.contains("<Resource>/bkt/k</Resource>"), "{}", r.body);
    // HEAD errors have no body, like S3's.
    s.add_fault(FaultRule::error(503, "SlowDown").operation("HeadObject")).unwrap();
    let r = common::send(s.addr(), common::req("HEAD", "/bkt/k")).await;
    assert_eq!((r.status, r.body.as_str()), (503, ""));
}

#[tokio::test]
async fn delay_holds_the_request_and_trips_client_timeouts() {
    let s = start().await;
    let s3 = client(&s);
    seed(&s3, "bkt", &["k"]).await;
    s.add_fault(FaultRule::delay(Duration::from_millis(300)).operation("HeadObject").times(1))
        .unwrap();
    let t = Instant::now();
    s3.head_object().bucket("bkt").key("k").send().await.unwrap();
    assert!(t.elapsed() >= Duration::from_millis(300), "{:?}", t.elapsed());
    let heads = log(&s, "HeadObject");
    assert_eq!(heads.len(), 1);
    let f = heads[0].fault.unwrap();
    assert_eq!((f.kind, f.delayed), (FaultKind::Delay, true));
    assert!(heads[0].latency >= Duration::from_millis(300));

    // A client attempt timeout shorter than the delay: the first attempt times out, the retry
    // gets through.
    s.control().clear_requests();
    s.add_fault(FaultRule::delay(Duration::from_secs(2)).operation("GetObject").times(1))
        .unwrap();
    let timeouts = TimeoutConfig::builder()
        .operation_attempt_timeout(Duration::from_millis(200))
        .build();
    let impatient = client_with(&s, 2, Some(timeouts));
    let t = Instant::now();
    let got = impatient.get_object().bucket("bkt").key("k").send().await.unwrap();
    assert_eq!(got.body.collect().await.unwrap().into_bytes().as_ref(), b"hello");
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());

    // A delay can come before an error, and can be random within bounds.
    let rule = FaultRule::internal_error()
        .with_delay(Delay::Between(Duration::from_millis(50), Duration::from_millis(100)))
        .operation("ListObjectsV2")
        .times(1);
    s.add_fault(rule).unwrap();
    let t = Instant::now();
    let err = no_retry(&s).list_objects_v2().bucket("bkt").send().await.unwrap_err();
    assert_eq!(err.into_service_error().code(), Some("InternalError"));
    assert!(t.elapsed() >= Duration::from_millis(50));
    let f = log(&s, "ListObjectsV2")[0].fault.unwrap();
    assert_eq!((f.kind, f.delayed), (FaultKind::Error, true));
}

// Multi-threaded: the raw exchange below blocks its thread while the server answers.
#[tokio::test(flavor = "multi_thread")]
async fn truncated_get_makes_the_client_fail() {
    let s = start().await;
    let s3 = client(&s);
    s3.create_bucket().bucket("bkt").send().await.unwrap();
    let data = vec![7u8; 10_000];
    s3.put_object()
        .bucket("bkt")
        .key("big")
        .body(ByteStream::from(data.clone()))
        .send()
        .await
        .unwrap();
    s.add_fault(FaultRule::truncate(100).operation("GetObject").times(1)).unwrap();

    let got = s3.get_object().bucket("bkt").key("big").send().await.unwrap();
    assert_eq!(got.content_length(), Some(10_000));
    assert!(got.body.collect().await.is_err(), "a cut body must not read as complete");
    let rec = &log(&s, "GetObject")[0];
    assert_eq!((rec.status, rec.fault.unwrap().kind), (200, FaultKind::Truncate));

    // The rule is used up: the next read is whole.
    let got = s3.get_object().bucket("bkt").key("big").send().await.unwrap();
    assert_eq!(got.body.collect().await.unwrap().into_bytes().as_ref(), &data[..]);

    // On the wire: the announced length, then only 100 body bytes before the server closes.
    s.add_fault(FaultRule::truncate(100).operation("GetObject").times(1)).unwrap();
    let raw = raw_exchange(
        &s,
        &format!(
            "GET /bkt/big HTTP/1.1\r\nHost: x\r\nAuthorization: {}\r\n\r\n",
            common::UNVERIFIED_AUTH
        ),
    );
    let text = String::from_utf8_lossy(&raw);
    let (head, body) = text.split_once("\r\n\r\n").unwrap();
    assert!(head.to_ascii_lowercase().contains("content-length: 10000"), "{head}");
    assert_eq!(body.len(), 100);
}

/// Sends raw bytes and reads until the server closes the connection.
fn raw_exchange(s: &Server, request: &str) -> Vec<u8> {
    let mut c = std::net::TcpStream::connect(s.addr()).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    c.write_all(request.as_bytes()).unwrap();
    let mut out = Vec::new();
    let _ = c.read_to_end(&mut out);
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn reset_drops_the_connection_and_the_sdk_retries() {
    let s = start().await;
    let s3 = client(&s);
    seed(&s3, "bkt", &[]).await;
    s.add_fault(FaultRule::reset().operation("PutObject").times(1)).unwrap();

    s3.put_object()
        .bucket("bkt")
        .key("k")
        .body(ByteStream::from_static(b"v"))
        .send()
        .await
        .unwrap();
    let puts = log(&s, "PutObject");
    assert_eq!(puts.iter().map(|r| r.status).collect::<Vec<_>>(), [0, 200]);
    assert_eq!(puts[0].fault.unwrap().kind, FaultKind::Reset);

    // The reset request was not handled: with retries off, the object is not written.
    s.add_fault(FaultRule::reset().operation("PutObject").times(1)).unwrap();
    let err = no_retry(&s)
        .put_object()
        .bucket("bkt")
        .key("other")
        .body(ByteStream::from_static(b"v"))
        .send()
        .await
        .unwrap_err();
    assert!(matches!(err, aws_sdk_s3::error::SdkError::DispatchFailure(_)), "{err:?}");
    assert!(s3.head_object().bucket("bkt").key("other").send().await.is_err());

    // On the wire it is a TCP reset, not an orderly close.
    s.add_fault(FaultRule::reset().times(1)).unwrap();
    let mut c = std::net::TcpStream::connect(s.addr()).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    c.write_all(b"GET /bkt/k HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
    let mut buf = [0u8; 64];
    let r = c.read(&mut buf);
    assert!(
        matches!(&r, Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset),
        "expected a reset, got {r:?}"
    );
}

#[tokio::test]
async fn rules_match_operation_bucket_and_key_prefix() {
    let s = start().await;
    let s3 = no_retry(&s);
    seed(&s3, "one", &["logs/a", "data/a"]).await;
    seed(&s3, "two", &["logs/a"]).await;
    s.add_fault(FaultRule::slow_down().operation("GetObject").bucket("one").key_prefix("logs/"))
        .unwrap();

    let get = |b: &'static str, k: &'static str| {
        let s3 = s3.clone();
        async move { s3.get_object().bucket(b).key(k).send().await.is_ok() }
    };
    assert!(!get("one", "logs/a").await);
    assert!(get("one", "data/a").await, "other prefix");
    assert!(get("two", "logs/a").await, "other bucket");
    assert!(s3.head_object().bucket("one").key("logs/a").send().await.is_ok(), "other operation");
    assert!(
        s3.list_objects_v2().bucket("one").prefix("logs/").send().await.is_ok(),
        "bucket-level request"
    );

    // Several operations in one rule; `skip` lets the first matches through.
    s.clear_faults();
    let mut rule = FaultRule::internal_error()
        .operation("HeadObject")
        .operation("DeleteObject")
        .skip(1);
    rule.bucket = Some("two".into());
    s.add_fault(rule).unwrap();
    assert!(s3.head_object().bucket("two").key("logs/a").send().await.is_ok(), "skipped");
    assert!(s3.head_object().bucket("two").key("logs/a").send().await.is_err());
    assert!(s3.delete_object().bucket("two").key("logs/a").send().await.is_err());
    assert!(get("two", "logs/a").await);

    // Rules are checked in order and the first that fires wins.
    s.clear_faults();
    s.add_fault(FaultRule::error(503, "ServiceUnavailable").bucket("one").times(1))
        .unwrap();
    s.add_fault(FaultRule::internal_error().bucket("one")).unwrap();
    for want in ["ServiceUnavailable", "InternalError"] {
        let err = s3.list_objects_v2().bucket("one").send().await.unwrap_err();
        assert_eq!(err.into_service_error().code(), Some(want));
    }
}

async fn coin_flips(s: &Server, seed: u64) -> Vec<bool> {
    s.clear_faults();
    s.add_fault(FaultRule::slow_down().operation("HeadObject").probability(0.5).seed(seed))
        .unwrap();
    let s3 = no_retry(s);
    let mut out = Vec::new();
    for _ in 0..40 {
        out.push(s3.head_object().bucket("bkt").key("k").send().await.is_ok());
    }
    out
}

#[tokio::test]
async fn seeded_probability_is_repeatable() {
    let s = start().await;
    seed(&client(&s), "bkt", &["k"]).await;
    let a = coin_flips(&s, 42).await;
    let b = coin_flips(&s, 42).await;
    assert_eq!(a, b);
    let failed = a.iter().filter(|ok| !**ok).count();
    assert!((8..=32).contains(&failed), "p=0.5 over 40 requests failed {failed}");
    assert_ne!(a, coin_flips(&s, 43).await);
    // Another server, same seed, same sequence.
    let other = start().await;
    seed(&client(&other), "bkt", &["k"]).await;
    assert_eq!(a, coin_flips(&other, 42).await);
    // A generated seed is reported, so a failing run can be replayed.
    s.clear_faults();
    let info = s.add_fault(FaultRule::slow_down().probability(0.3)).unwrap();
    assert!(info.rule.seed.is_some());
}

#[tokio::test]
async fn reset_clears_rules_unless_asked_not_to() {
    let s = start().await;
    s.add_fault(FaultRule::slow_down()).unwrap();
    s.add_fault(FaultRule::reset().bucket("x")).unwrap();
    s.control().reset_with(ResetOptions {
        faults: false,
        ..Default::default()
    });
    assert_eq!(s.faults().len(), 2);
    s.reset();
    assert!(s.faults().is_empty());
    client(&s).create_bucket().bucket("bkt").send().await.unwrap();
    assert_eq!(log(&s, "CreateBucket")[0].status, 200);
    // Snapshots do not carry rules: restoring leaves the current rules alone.
    s.snapshot(Some("base")).unwrap();
    s.add_fault(FaultRule::slow_down().operation("GetObject")).unwrap();
    s.restore("base").unwrap();
    assert_eq!(s.faults().len(), 1);
}

// ---- HTTP API and CLI ----

struct Reply {
    status: u16,
    body: Value,
}

async fn api(s: &Server, method: &str, path: &str, body: Value) -> Reply {
    let stream = TcpStream::connect(s.addr()).await.unwrap();
    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream)).await.unwrap();
    tokio::spawn(conn);
    let payload = if body.is_null() { String::new() } else { body.to_string() };
    let req = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", s.addr().to_string())
        .body(Full::new(Bytes::from(payload)))
        .unwrap();
    let resp = sender.send_request(req).await.unwrap();
    let status = resp.status().as_u16();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    Reply {
        status,
        body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    }
}

#[tokio::test]
async fn http_api_adds_lists_and_removes_rules() {
    let s = start().await;
    let s3 = client(&s);
    seed(&s3, "bkt", &["k"]).await;

    let r = api(
        &s,
        "POST",
        "/_fakes3/faults",
        json!({ "operation": "GetObject", "bucket": "bkt", "times": 1, "error": "SlowDown" }),
    )
    .await;
    assert_eq!(r.status, 201, "{}", r.body);
    let id = r.body["id"].as_u64().unwrap();
    assert_eq!(r.body["error"], json!({ "status": 503, "code": "SlowDown", "message": null }));
    assert_eq!(r.body["remaining"], 1);
    assert_eq!(r.body["operation"], json!(["GetObject"]));

    let r = api(
        &s,
        "POST",
        "/_fakes3/faults",
        json!({ "key_prefix": "slow/", "delay": { "min": "10ms", "max": 20 }, "probability": 0.5, "seed": 9 }),
    )
    .await;
    assert_eq!(r.status, 201, "{}", r.body);
    assert_eq!(r.body["delay"], json!({ "min": 10.0, "max": 20.0 }));
    assert_eq!(r.body["seed"], 9);
    let second = r.body["id"].as_u64().unwrap();

    s3.get_object().bucket("bkt").key("k").send().await.unwrap();
    let list = api(&s, "GET", "/_fakes3/faults", Value::Null).await.body;
    let first = &list["faults"][0];
    assert_eq!((first["fired"].clone(), first["remaining"].clone()), (json!(1), json!(0)));
    let reqs = api(&s, "GET", "/_fakes3/requests?op=GetObject", Value::Null).await.body;
    assert_eq!(reqs["requests"][0]["status"], 503);
    assert_eq!(reqs["requests"][0]["fault"], json!({ "id": id, "type": "error", "delayed": false }));
    assert_eq!(reqs["requests"][1]["fault"], Value::Null);

    assert_eq!(api(&s, "GET", &format!("/_fakes3/faults/{second}"), Value::Null).await.status, 200);
    assert_eq!(api(&s, "DELETE", &format!("/_fakes3/faults/{id}"), Value::Null).await.status, 200);
    assert_eq!(api(&s, "DELETE", &format!("/_fakes3/faults/{id}"), Value::Null).await.status, 404);
    assert_eq!(s.faults().len(), 1);
    let r = api(&s, "DELETE", "/_fakes3/faults", Value::Null).await;
    assert_eq!(r.body, json!({ "cleared": 1 }));

    for bad in [
        json!({}),
        json!({ "error": "NoSuchThing" }),
        json!({ "reset": true, "truncate": 1 }),
        json!({ "delay": "-1s" }),
        json!({ "delay": "1s", "probability": 2 }),
        json!({ "reset": true, "colour": "red" }),
    ] {
        let r = api(&s, "POST", "/_fakes3/faults", bad.clone()).await;
        assert_eq!(r.status, 400, "{bad} -> {}", r.body);
    }

    // `reset` clears rules by default; `faults=false` keeps them.
    api(&s, "POST", "/_fakes3/faults", json!({ "reset": true })).await;
    api(&s, "POST", "/_fakes3/reset?faults=false", Value::Null).await;
    assert_eq!(s.faults().len(), 1);
    let status = api(&s, "GET", "/_fakes3", Value::Null).await.body;
    assert_eq!(status["faults"], 1);
    // Control-plane requests are never hit, even by a rule that matches everything.
    api(&s, "POST", "/_fakes3/reset", Value::Null).await;
    assert!(s.faults().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_manages_rules() {
    let s = std::sync::Arc::new(start().await);
    let run = |args: Vec<&'static str>| {
        let s = s.clone();
        async move {
            tokio::task::spawn_blocking(move || {
                let out = std::process::Command::new(env!("CARGO_BIN_EXE_fakes3"))
                    .args(["ctl", "--endpoint", &s.endpoint()])
                    .args(args)
                    .env_remove("FAKES3_ENDPOINT")
                    .output()
                    .unwrap();
                (
                    out.status.success(),
                    String::from_utf8_lossy(&out.stdout).into_owned(),
                    String::from_utf8_lossy(&out.stderr).into_owned(),
                )
            })
            .await
            .unwrap()
        }
    };
    let (ok, out, err) = run(vec!["faults", "add", "--op", "PutObject", "--error", "SlowDown", "--times", "1"]).await;
    assert!(ok && out.starts_with("added") && out.contains("503 SlowDown"), "{out}{err}");
    let (ok, out, err) = run(vec!["faults", "add", "--delay", "10ms..20ms", "--bucket", "b", "--reset"]).await;
    assert!(ok && out.contains("reset") && out.contains("bucket=b"), "{out}{err}");
    let (ok, out, _) = run(vec![
        "faults",
        "add",
        "--status",
        "500",
        "--key-prefix",
        "x/",
        "--probability",
        "0.25",
        "--seed",
        "5",
    ])
    .await;
    assert!(ok && out.contains("500 InternalError") && out.contains("seed=5"), "{out}");
    let (ok, out, _) = run(vec!["faults", "add", "--truncate", "10", "--op", "GetObject"]).await;
    assert!(ok && out.contains("truncate after 10 bytes"), "{out}");
    let (ok, _, err) = run(vec!["faults", "add", "--op", "GetObject"]).await;
    assert!(!ok && err.contains("needs"), "{err}");

    let (ok, out, _) = run(vec!["faults", "list"]).await;
    assert!(ok && out.lines().count() == 4, "{out}");
    let (ok, out, _) = run(vec!["--json", "faults", "list"]).await;
    let v: Value = serde_json::from_str(&out).unwrap();
    assert!(ok && v["faults"][1]["delay"] == json!({ "min": 10.0, "max": 20.0 }), "{out}");
    let first = v["faults"][0]["id"].as_u64().unwrap().to_string();
    let first: &'static str = Box::leak(first.into_boxed_str());
    let (ok, out, _) = run(vec!["faults", "delete", first]).await;
    assert!(ok && out.contains("deleted"), "{out}");
    let (ok, _, err) = run(vec!["faults", "delete", first]).await;
    assert!(!ok && err.contains("no fault rule"), "{err}");
    let (ok, out, _) = run(vec!["status"]).await;
    assert!(ok && out.contains("fault rules    3"), "{out}");
    let (ok, _, _) = run(vec!["reset", "--keep-faults"]).await;
    assert!(ok && s.faults().len() == 3);
    let (ok, out, _) = run(vec!["faults", "clear"]).await;
    assert!(ok && out.contains("cleared 3"), "{out}");

    // Requests hit by a rule are marked in `requests list`.
    s.add_fault(FaultRule::slow_down().times(1)).unwrap();
    let r = common::send(s.addr(), common::req("PUT", "/bkt")).await;
    assert_eq!(r.status, 503);
    let (ok, out, _) = run(vec!["requests", "list"]).await;
    assert!(ok && out.contains("[fault error #"), "{out}");
}
