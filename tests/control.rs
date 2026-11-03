//! End-to-end tests of the control plane: HTTP API, `fakes3 ctl` and the Rust methods, against
//! a real in-process server (signatures are not verified, so a made-up `Authorization` header is enough).

use std::time::Duration;

use bytes::Bytes;
use fakes3::{Config, RequestFilter, Server};
use http_body_util::{BodyExt, Full};
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use tokio::net::TcpStream;

mod common;

async fn start() -> Server {
    start_with(Config::default()).await
}

async fn start_with(config: Config) -> Server {
    Server::start(config).await.unwrap()
}

struct Reply {
    status: u16,
    headers: http::HeaderMap,
    body: String,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("not JSON ({e}): {}", self.body))
    }
}

async fn send(s: &Server, method: &str, path: &str, body: &str) -> Reply {
    send_with(s, method, path, body, &[]).await
}

async fn send_with(s: &Server, method: &str, path: &str, body: &str, headers: &[(&str, &str)]) -> Reply {
    let stream = TcpStream::connect(s.addr()).await.unwrap();
    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream)).await.unwrap();
    tokio::spawn(conn);
    let mut req = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", s.addr().to_string())
        .header("authorization", common::UNVERIFIED_AUTH);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let resp = sender
        .send_request(req.body(Full::new(Bytes::from(body.to_owned()))).unwrap())
        .await
        .unwrap();
    let (parts, body) = resp.into_parts();
    let body = String::from_utf8(body.collect().await.unwrap().to_bytes().to_vec()).unwrap();
    Reply {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

async fn put_object(s: &Server, bucket: &str, key: &str, body: &str) {
    assert_eq!(send(s, "PUT", &format!("/{bucket}"), "").await.status / 100, 2);
    assert_eq!(send(s, "PUT", &format!("/{bucket}/{key}"), body).await.status, 200);
}

#[tokio::test]
async fn reset_clears_buckets_log_and_clock_but_keeps_snapshots() {
    let s = start().await;
    put_object(&s, "bkt", "k", "v").await;
    s.snapshot(Some("keep")).unwrap();
    s.clock()
        .freeze(Some(std::time::UNIX_EPOCH + Duration::from_secs(1_900_000_000)))
        .unwrap();

    let r = send(&s, "POST", "/_fakes3/reset", "").await;
    assert_eq!((r.status, r.json()["reset"].clone()), (200, json!(true)));

    assert_eq!(send(&s, "GET", "/bkt/k", "").await.status, 404);
    assert_eq!(send(&s, "GET", "/", "").await.body.matches("<Name>").count(), 0);
    assert!(!s.clock().is_virtual());
    // Only the two requests made after the reset are in the log.
    assert_eq!(s.requests(&RequestFilter::default()).1, 2);
    assert_eq!(
        send(&s, "GET", "/_fakes3/snapshots", "").await.json()["snapshots"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // Opt out of parts of the reset.
    s.clock().freeze(None).unwrap();
    send(&s, "POST", "/_fakes3/reset?clock=false&requests=false&snapshots=true", "").await;
    assert!(s.clock().is_frozen());
    assert_eq!(s.requests(&RequestFilter::default()).1, 2);
    assert!(s.control().snapshots().is_empty());
    assert_eq!(send(&s, "POST", "/_fakes3/reset?clock=maybe", "").await.status, 400);
    s.shutdown().await;
}

#[tokio::test]
async fn snapshot_and_restore_round_trip() {
    let s = start().await;
    put_object(&s, "bkt", "keep", "one").await;
    send(
        &s,
        "PUT",
        "/bkt?versioning",
        r#"<VersioningConfiguration xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><Status>Enabled</Status></VersioningConfiguration>"#,
    )
    .await;
    let up = send(&s, "POST", "/bkt/mp?uploads", "").await;
    assert_eq!(up.status, 200);

    let r = send(&s, "POST", "/_fakes3/snapshots", r#"{"name":"base"}"#).await;
    assert_eq!(r.status, 201);
    let j = r.json();
    assert_eq!(
        (j["name"].clone(), j["buckets"].clone(), j["objects"].clone(), j["uploads"].clone()),
        (json!("base"), json!(1), json!(1), json!(1))
    );

    // Change everything, then roll back.
    send(&s, "PUT", "/bkt/keep", "two").await;
    send(&s, "DELETE", "/bkt/keep", "").await;
    put_object(&s, "other", "x", "y").await;
    assert_eq!(send(&s, "GET", "/bkt/keep", "").await.status, 404);

    let r = send(&s, "POST", "/_fakes3/snapshots/base/restore", "").await;
    assert_eq!(r.status, 200);
    let got = send(&s, "GET", "/bkt/keep", "").await;
    assert_eq!((got.status, got.body.as_str()), (200, "one"));
    assert_eq!(send(&s, "HEAD", "/other", "").await.status, 404);
    // Versioning came back too, and so did the multipart upload.
    let put = send(&s, "PUT", "/bkt/keep", "three").await;
    assert!(put.headers.contains_key("x-amz-version-id"));
    assert!(send(&s, "GET", "/bkt?uploads", "").await.body.contains("<Key>mp</Key>"));

    // The snapshot survives a restore and can be used again.
    send(&s, "DELETE", "/bkt/keep", "").await;
    s.restore("base").unwrap();
    assert_eq!(send(&s, "GET", "/bkt/keep", "").await.body, "one");
    s.shutdown().await;
}

#[tokio::test]
async fn snapshot_names_and_errors() {
    let s = start().await;
    let a = send(&s, "POST", "/_fakes3/snapshots", "").await.json();
    let b = send(&s, "POST", "/_fakes3/snapshots", "{}").await.json();
    assert_ne!(a["name"], b["name"]);
    assert_eq!(send(&s, "POST", "/_fakes3/snapshots", r#"{"name":"x"}"#).await.status, 201);
    assert_eq!(send(&s, "POST", "/_fakes3/snapshots", r#"{"name":"x"}"#).await.status, 409);
    assert_eq!(send(&s, "POST", "/_fakes3/snapshots", r#"{"name":"a/b"}"#).await.status, 400);
    assert_eq!(send(&s, "POST", "/_fakes3/snapshots", r#"{"nme":"a"}"#).await.status, 400);
    assert_eq!(send(&s, "POST", "/_fakes3/snapshots", "not json").await.status, 400);
    assert_eq!(send(&s, "POST", "/_fakes3/snapshots/missing/restore", "").await.status, 404);
    assert_eq!(send(&s, "DELETE", "/_fakes3/snapshots/missing", "").await.status, 404);
    assert_eq!(
        send(&s, "GET", "/_fakes3/snapshots", "").await.json()["snapshots"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(send(&s, "DELETE", "/_fakes3/snapshots/x", "").await.status, 200);
    assert_eq!(
        send(&s, "GET", "/_fakes3/snapshots", "").await.json()["snapshots"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    s.shutdown().await;
}

#[tokio::test]
async fn clock_endpoints_move_object_times() {
    let s = start().await;
    let c = send(&s, "GET", "/_fakes3/clock", "").await.json();
    assert_eq!((c["frozen"].clone(), c["shifted"].clone()), (json!(false), json!(false)));

    let c = send(&s, "POST", "/_fakes3/clock/freeze", r#"{"at":"2030-01-02T03:04:05Z"}"#)
        .await
        .json();
    assert_eq!(
        (c["now"].clone(), c["frozen"].clone()),
        (json!("2030-01-02T03:04:05.000Z"), json!(true))
    );
    put_object(&s, "bkt", "k", "v").await;
    let head = send(&s, "HEAD", "/bkt/k", "").await;
    assert_eq!(head.headers["last-modified"], "Wed, 02 Jan 2030 03:04:05 GMT");

    let c = send(&s, "POST", "/_fakes3/clock/advance", r#"{"by":"30d"}"#).await.json();
    assert_eq!(c["now"], "2030-02-01T03:04:05.000Z");
    let c = send(&s, "POST", "/_fakes3/clock/advance", r#"{"by":"-1h30m"}"#).await.json();
    assert_eq!(c["now"], "2030-02-01T01:34:05.000Z");

    let c = send(&s, "POST", "/_fakes3/clock/set", r#"{"at":"2031-05-06T07:08:09Z"}"#)
        .await
        .json();
    assert_eq!(
        (c["now"].clone(), c["frozen"].clone()),
        (json!("2031-05-06T07:08:09.000Z"), json!(true))
    );

    let c = send(&s, "POST", "/_fakes3/clock/unfreeze", "").await.json();
    assert_eq!((c["frozen"].clone(), c["shifted"].clone()), (json!(false), json!(true)));
    assert!(c["now"].as_str().unwrap().starts_with("2031-05-06T07:08:"));

    let c = send(&s, "POST", "/_fakes3/clock/freeze", "").await.json();
    assert_eq!(c["frozen"], true);
    let c = send(&s, "POST", "/_fakes3/clock/reset", "").await.json();
    assert_eq!((c["frozen"].clone(), c["shifted"].clone()), (json!(false), json!(false)));

    for (path, body) in [
        ("freeze", r#"{"at":"yesterday"}"#),
        ("set", "{}"),
        ("advance", "{}"),
        ("advance", r#"{"by":"5 fortnights"}"#),
        ("advance", r#"{"by":"99999y"}"#),
        ("freeze", r#"{"at":"9999-01-01T00:00:00Z"}"#),
    ] {
        assert_eq!(
            send(&s, "POST", &format!("/_fakes3/clock/{path}"), body).await.status,
            400,
            "{path} {body}"
        );
    }
    s.shutdown().await;
}

#[tokio::test]
async fn expired_presigned_urls_follow_the_server_clock() {
    let s = start().await;
    put_object(&s, "bkt", "k", "v").await;
    send(&s, "POST", "/_fakes3/clock/freeze", r#"{"at":"2030-01-01T00:00:00Z"}"#).await;
    let url = "/bkt/k?X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Date=20300101T000000Z&X-Amz-Expires=3600&X-Amz-Signature=00";
    // Still valid by the server clock: whatever happens next is up to s3s, but not our expiry error.
    assert_ne!(send(&s, "GET", url, "").await.status, 403);
    send(&s, "POST", "/_fakes3/clock/advance", r#"{"by":"2h"}"#).await;
    let r = send(&s, "GET", url, "").await;
    assert_eq!(r.status, 403);
    assert!(r.body.contains("Request has expired"));
    s.shutdown().await;
}

#[tokio::test]
async fn request_log_records_and_filters() {
    let s = start().await;
    send(&s, "PUT", "/logs", "").await;
    send(&s, "PUT", "/logs/a/one.txt", "hello").await;
    send(&s, "PUT", "/logs/b/two.txt", "hi").await;
    send(&s, "GET", "/logs/a/one.txt", "").await;
    send(&s, "GET", "/logs/missing", "").await;
    send(&s, "GET", "/logs?list-type=2&prefix=a%2F", "").await;
    send(&s, "PUT", "/logs/copy", "").await;
    send_with(&s, "PUT", "/logs/c%20d", "", &[("x-amz-copy-source", "/logs/a/one.txt")]).await;
    send(&s, "GET", "/_fakes3/status", "").await; // control-plane calls are not logged

    let all = send(&s, "GET", "/_fakes3/requests", "").await.json();
    assert_eq!(all["total"], 8);
    let rows = all["requests"].as_array().unwrap();
    let ops: Vec<_> = rows.iter().map(|r| r["operation"].as_str().unwrap()).collect();
    assert_eq!(
        ops,
        [
            "CreateBucket",
            "PutObject",
            "PutObject",
            "GetObject",
            "GetObject",
            "ListObjectsV2",
            "PutObject",
            "CopyObject"
        ]
    );
    let seqs: Vec<_> = rows.iter().map(|r| r["seq"].as_u64().unwrap()).collect();
    assert!(seqs.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(rows[1]["bucket"], "logs");
    assert_eq!(rows[1]["key"], "a/one.txt");
    assert_eq!(
        (rows[1]["method"].clone(), rows[1]["status"].clone(), rows[1]["bytes_in"].clone()),
        (json!("PUT"), json!(200), json!(5))
    );
    assert_eq!(rows[3]["bytes_out"], 5);
    assert_eq!(rows[7]["key"], "c d");
    assert!(rows[1]["time"].as_str().unwrap().ends_with('Z'));
    assert!(rows[1]["latency_us"].is_u64());

    let q = |query: &str| {
        let s = &s;
        let path = format!("/_fakes3/requests?{query}");
        async move { send(s, "GET", &path, "").await.json() }
    };
    assert_eq!(q("op=PutObject").await["total"], 3);
    assert_eq!(q("op=getobject&status=404").await["total"], 1);
    assert_eq!(q("status=4xx").await["total"], 1);
    assert_eq!(q("status=error").await["total"], 1);
    assert_eq!(q("status=200").await["total"], 7);
    assert_eq!(q("method=get").await["total"], 3);
    assert_eq!(q("key_prefix=a%2F").await["total"], 2);
    assert_eq!(q("bucket=nope").await["total"], 0);
    assert_eq!(q("after=6").await["total"], 2);
    assert_eq!(q("since=2000-01-01T00:00:00Z").await["total"], 8);
    assert_eq!(q("since=2999-01-01T00:00:00Z").await["total"], 0);
    // `limit` keeps the most recent matches and `total` still counts them all.
    let l = q("op=PutObject&limit=2").await;
    assert_eq!(
        (
            l["total"].clone(),
            l["requests"].as_array().unwrap().len(),
            l["requests"][0]["seq"].clone()
        ),
        (json!(3), 2, json!(3))
    );
    assert_eq!(send(&s, "GET", "/_fakes3/requests?status=nope", "").await.status, 400);
    assert_eq!(send(&s, "GET", "/_fakes3/requests?bogus=1", "").await.status, 400);

    // The Rust API sees the same records.
    let (rows, total) = s.requests(&RequestFilter {
        operation: Some("GetObject".into()),
        ..Default::default()
    });
    assert_eq!((rows.len(), total), (2, 2));
    assert_eq!(rows[0].operation(), "GetObject");

    assert_eq!(send(&s, "DELETE", "/_fakes3/requests", "").await.status, 200);
    assert_eq!(q("").await["total"], 0);
    send(&s, "GET", "/", "").await;
    // Sequence numbers keep counting after a clear.
    assert_eq!(q("").await["requests"][0]["seq"], 9);
    s.shutdown().await;
}

#[tokio::test]
async fn request_log_is_bounded_and_can_be_disabled() {
    let s = start_with(Config {
        request_log_capacity: 3,
        ..Config::default()
    })
    .await;
    for _ in 0..5 {
        send(&s, "GET", "/", "").await;
    }
    let j = send(&s, "GET", "/_fakes3/requests", "").await.json();
    assert_eq!(
        (j["total"].clone(), j["dropped"].clone(), j["requests"][0]["seq"].clone()),
        (json!(3), json!(2), json!(3))
    );

    let c = send(&s, "PUT", "/_fakes3/requests/config", r#"{"enabled":false}"#).await.json();
    assert_eq!((c["enabled"].clone(), c["capacity"].clone()), (json!(false), json!(3)));
    send(&s, "GET", "/", "").await;
    assert_eq!(send(&s, "GET", "/_fakes3/requests", "").await.json()["requests"][2]["seq"], 5);

    let c = send(&s, "PUT", "/_fakes3/requests/config", r#"{"enabled":true,"capacity":2}"#)
        .await
        .json();
    assert_eq!(
        (c["enabled"].clone(), c["capacity"].clone(), c["stored"].clone()),
        (json!(true), json!(2), json!(2))
    );
    assert_eq!(send(&s, "PUT", "/_fakes3/requests/config", r#"{"capacity":0}"#).await.status, 400);
    assert_eq!(send(&s, "GET", "/_fakes3/requests/config", "").await.status, 200);
    s.shutdown().await;

    let off = start_with(Config {
        request_log: false,
        ..Config::default()
    })
    .await;
    send(&off, "GET", "/", "").await;
    assert_eq!(send(&off, "GET", "/_fakes3/requests", "").await.json()["total"], 0);
    off.shutdown().await;
}

#[tokio::test]
async fn api_edges() {
    let s = start().await;
    let st = send(&s, "GET", "/_fakes3", "").await.json();
    assert_eq!(st["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(st["auth"], "any-credentials");
    let strict = start_with(Config {
        auth: true,
        ..Config::default()
    })
    .await;
    assert_eq!(send(&strict, "GET", "/_fakes3", "").await.json()["auth"], "strict");
    assert_eq!(send(&s, "GET", "/_fakes3/", "").await.status, 200);
    assert_eq!(send(&s, "GET", "/_fakes3/nope", "").await.status, 404);
    assert_eq!(send(&s, "POST", "/_fakes3/clock/nope", "").await.status, 404);
    assert_eq!(send(&s, "GET", "/_fakes3/reset", "").await.status, 405);
    assert_eq!(send(&s, "DELETE", "/_fakes3/clock", "").await.status, 405);
    // Browsers attach Origin to cross-site requests; those must not be able to reset the server.
    put_object(&s, "bkt", "k", "v").await;
    let r = send_with(&s, "POST", "/_fakes3/reset", "", &[("origin", "http://evil.example")]).await;
    assert_eq!(r.status, 403);
    assert_eq!(send(&s, "GET", "/bkt/k", "").await.status, 200);
    // Errors are JSON with a code and a message.
    let e = send(&s, "POST", "/_fakes3/snapshots/zzz/restore", "").await.json();
    assert_eq!(e["error"]["code"], "NotFound");
    // A bucket named like the prefix is not a valid S3 bucket, so S3 paths never reach the API.
    assert_eq!(send(&s, "PUT", "/_notabucket", "").await.status, 400);
    s.shutdown().await;
}

#[tokio::test]
async fn servers_are_independent() {
    let (a, b) = (start().await, start().await);
    put_object(&a, "only-a", "k", "v").await;
    a.clock().freeze(None).unwrap();
    assert_eq!(send(&b, "GET", "/", "").await.body.matches("<Name>").count(), 0);
    assert!(!b.clock().is_frozen());
    b.reset();
    assert_eq!(send(&a, "GET", "/only-a/k", "").await.status, 200);
    a.shutdown().await;
    b.shutdown().await;
}

fn ctl(s: &Server, args: &[&str]) -> (bool, String, String) {
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
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_drives_the_api() {
    let s = std::sync::Arc::new(start().await);
    let run = |args: &'static [&'static str]| {
        let s = s.clone();
        async move { tokio::task::spawn_blocking(move || ctl(&s, args)).await.unwrap() }
    };
    put_object(&s, "bkt", "k", "v").await;

    let (ok, out, _) = run(&["status"]).await;
    assert!(
        ok && out.contains("1 buckets, 1 objects") && out.contains("legacy defaults"),
        "{out}"
    );

    let (ok, out, _) = run(&["snapshot", "create", "base"]).await;
    assert!(ok && out.starts_with("created base"), "{out}");
    let (ok, out, err) = run(&["snapshot", "create", "base"]).await;
    assert!(!ok && err.contains("already exists"), "{out}{err}");

    let (ok, out, _) = run(&["clock", "freeze", "2030-01-02T03:04:05Z"]).await;
    assert!(ok && out.contains("2030-01-02T03:04:05.000Z (frozen)"), "{out}");
    let (ok, out, _) = run(&["clock", "advance", "30d"]).await;
    assert!(ok && out.contains("2030-02-01T03:04:05.000Z"), "{out}");
    let (ok, out, _) = run(&["clock", "advance", "-1d"]).await;
    assert!(ok && out.contains("2030-01-31T03:04:05.000Z"), "{out}");
    let (ok, out, _) = run(&["--json", "clock", "get"]).await;
    assert!(ok && serde_json::from_str::<Value>(&out).unwrap()["frozen"] == true, "{out}");

    let (ok, out, _) = run(&["requests", "list", "--op", "PutObject", "--bucket", "bkt"]).await;
    assert!(ok && out.contains("PutObject") && out.contains("/bkt/k"), "{out}");
    let (ok, out, _) = run(&["requests", "list", "--count"]).await;
    assert_eq!(out.trim(), "2", "ok={ok}");
    let (ok, out, _) = run(&["requests", "config", "--disable"]).await;
    assert!(ok && out.contains("disabled"), "{out}");
    let (ok, out, _) = run(&["requests", "clear"]).await;
    assert!(ok && out.contains("cleared"));

    let (ok, _, _) = run(&["reset"]).await;
    assert!(ok);
    assert_eq!(send(&s, "HEAD", "/bkt", "").await.status, 404);
    let (ok, out, _) = run(&["snapshot", "restore", "base"]).await;
    assert!(ok && out.starts_with("restored base"), "{out}");
    assert_eq!(send(&s, "GET", "/bkt/k", "").await.body, "v");
    let (ok, out, _) = run(&["snapshot", "list"]).await;
    assert!(ok && out.contains("base"), "{out}");
    let (ok, out, _) = run(&["snapshot", "delete", "base"]).await;
    assert!(ok && out.contains("deleted base"), "{out}");
    let (ok, _, err) = run(&["snapshot", "restore", "base"]).await;
    assert!(!ok && err.contains("no snapshot named"), "{err}");
}

#[test]
fn cli_reports_unreachable_server() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_fakes3"))
        .args(["ctl", "--endpoint", "http://127.0.0.1:1", "status"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("cannot connect"));
}

/// Waits for `used` to reach `want`: buffers are freed when the last reference drops, which for a
/// finished request can be a moment after the client saw the response.
async fn expect_memory(s: &Server, want: u64) {
    for _ in 0..200 {
        if s.memory().used == want {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(s.memory().used, want);
}

#[tokio::test]
async fn reset_releases_accounted_memory() {
    let s = start().await;
    assert_eq!(s.memory().used, 0);
    let big = "x".repeat(100_000);
    put_object(&s, "bkt", "one", &big).await;
    assert_eq!(send(&s, "PUT", "/bkt/two", &big).await.status, 200);
    assert!(s.memory().used >= 200_000);
    // Multipart parts are accounted too.
    let up = send(&s, "POST", "/bkt/mp?uploads", "").await.body;
    let id = up.split("<UploadId>").nth(1).unwrap().split("</UploadId>").next().unwrap();
    assert_eq!(
        send(&s, "PUT", &format!("/bkt/mp?partNumber=1&uploadId={id}"), &big).await.status,
        200
    );
    assert!(s.memory().used >= 300_000);

    assert_eq!(send(&s, "POST", "/_fakes3/reset", "").await.status, 200);
    expect_memory(&s, 0).await;
    s.shutdown().await;
}

#[tokio::test]
async fn snapshots_keep_data_alive_until_deleted() {
    let s = start().await;
    let big = "x".repeat(100_000);
    put_object(&s, "bkt", "k", &big).await;
    let held = s.memory().used;
    assert!(held >= 100_000);
    s.snapshot(Some("a")).unwrap();

    // The snapshot shares the buffer instead of copying it.
    assert_eq!(s.memory().used, held);
    // Overwrite and delete: the old data is still referenced by the snapshot.
    send(&s, "PUT", "/bkt/k", &"y".repeat(50_000)).await;
    assert_eq!(s.memory().used, held + 50_000);
    send(&s, "DELETE", "/bkt/k", "").await;
    assert_eq!(s.memory().used, held);
    s.reset();
    expect_memory(&s, held).await;

    // Restoring brings the data back without any new allocation.
    s.restore("a").unwrap();
    assert_eq!(s.memory().used, held);
    assert_eq!(send(&s, "GET", "/bkt/k", "").await.body, big);

    // Rolling back to an empty snapshot frees what only the live store was holding.
    s.reset();
    s.snapshot(Some("empty")).unwrap();
    s.restore("a").unwrap();
    s.restore("empty").unwrap();
    s.control().delete_snapshot("a").unwrap();
    expect_memory(&s, 0).await;
    s.shutdown().await;
}

#[tokio::test]
async fn stats_endpoint_and_cli_report_memory() {
    let s = std::sync::Arc::new(
        start_with(Config {
            memory_limit: fakes3::MemoryLimit::Bytes(10_000_000),
            ..Config::default()
        })
        .await,
    );
    put_object(&s, "bkt", "k", &"x".repeat(100_000)).await;
    let j = send(&s, "GET", "/_fakes3/stats", "").await.json();
    assert_eq!(j["memory"]["limit"], 10_000_000);
    assert_eq!(j["memory"]["used"], s.memory().used);
    assert!(j["memory"]["used"].as_u64().unwrap() >= 100_000);
    assert_eq!((j["store"]["buckets"].clone(), j["store"]["objects"].clone()), (json!(1), json!(1)));
    assert_eq!(send(&s, "GET", "/_fakes3", "").await.json()["memory"], j["memory"]);

    let s2 = s.clone();
    let (ok, out, _) = tokio::task::spawn_blocking(move || ctl(&s2, &["stats"])).await.unwrap();
    assert!(
        ok && out.contains("memory") && out.contains("of 9.5 MiB") && out.contains("1 buckets, 1 objects"),
        "{out}"
    );
}

#[tokio::test]
async fn rust_api_creates_buckets_like_create_bucket() {
    let s = start_with(Config {
        bucket_defaults: fakes3::BucketDefaults::Aws,
        users: vec!["alt:ALTKEY:ALTSECRET".parse().unwrap()],
        ..Config::default()
    })
    .await;
    s.create_bucket("made").unwrap();
    // A bucket the default account already has is left alone, as CreateBucket does in us-east-1.
    put_object(&s, "made", "k", "v").await;
    s.create_bucket("made").unwrap();
    assert_eq!(send(&s, "GET", "/made/k", "").await.body, "v");
    // The bucket defaults apply, and the default account owns the bucket.
    let own = send(&s, "GET", "/made?ownershipControls", "").await.body;
    assert!(own.contains("BucketOwnerEnforced"), "{own}");
    let list = send(&s, "GET", "/", "").await.body;
    assert!(list.contains("<Name>made</Name>"), "{list}");

    let taken = common::send(s.addr(), common::req("PUT", "/taken").signed_by("ALTKEY", "ALTSECRET")).await;
    assert_eq!(taken.status, 200);
    assert!(matches!(s.create_bucket("taken"), Err(fakes3::ControlError::Conflict(_))));
    assert!(matches!(s.create_bucket("_fakes3"), Err(fakes3::ControlError::BadRequest(_))));
    assert!(matches!(s.create_bucket("Upper"), Err(fakes3::ControlError::BadRequest(_))));
    s.shutdown().await;
}

#[tokio::test]
async fn rust_api_enables_versioning() {
    let s = start().await;
    assert!(matches!(s.enable_versioning("missing"), Err(fakes3::ControlError::NotFound(_))));
    s.create_bucket("ver").unwrap();
    assert!(!send(&s, "GET", "/ver?versioning", "").await.body.contains("<Status>"));
    s.enable_versioning("ver").unwrap();
    assert!(
        send(&s, "GET", "/ver?versioning", "")
            .await
            .body
            .contains("<Status>Enabled</Status>")
    );
    let put = send(&s, "PUT", "/ver/k", "1").await;
    assert!(put.headers.contains_key("x-amz-version-id"));
    s.shutdown().await;
}
