//! Where fakes3 answers like S3 although s3s does not (docs/s3s-workarounds.md): SigV2 dates and
//! canonical resources under `--auth`, POST policy violations, and uploads without a length.

mod common;

use std::net::SocketAddr;
use std::time::{Duration, SystemTime};

use common::{Resp, http_date, req, send, v2_authorization, v2_signature};
use fakes3::{Config, Server};

async fn start(auth: bool) -> Server {
    Server::start(Config { auth, ..Config::default() }).await.unwrap()
}

/// A SigV2 header-signed request. `resource` is what the client puts in the CanonicalizedResource.
async fn v2(addr: SocketAddr, method: &str, target: &str, resource: &str, body: &[u8]) -> Resp {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let date = http_date(SystemTime::now());
    let auth = v2_authorization(method, "", &date, &[], resource);
    send(
        addr,
        req(method, path)
            .query(query)
            .anon()
            .header("authorization", &auth)
            .header("date", &date)
            .body(body),
    )
    .await
}

#[tokio::test]
async fn v2_bucket_level_requests_verify_with_or_without_the_slash() {
    let server = start(true).await;
    let a = server.addr();

    // botocore signs /bkt/ for the request line /bkt; other clients sign /bkt.
    let r = v2(a, "PUT", "/bkt", "/bkt/", b"").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = v2(a, "PUT", "/other", "/other", b"").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = v2(a, "PUT", "/bkt/k", "/bkt/k", b"v").await;
    assert_eq!(r.status, 200, "{}", r.body);
    for resource in ["/bkt/?acl", "/bkt?acl"] {
        let r = v2(a, "GET", "/bkt?acl", resource, b"").await;
        assert_eq!(r.status, 200, "{resource}: {}", r.body);
    }
    assert_eq!(v2(a, "HEAD", "/bkt", "/bkt/", b"").await.status, 200);

    // A signature for another resource, or another secret, is still refused.
    let r = v2(a, "GET", "/bkt?acl", "/other/?acl", b"").await;
    assert_eq!((r.status, r.code()), (403, "SignatureDoesNotMatch"), "{}", r.body);
    let r = v2(a, "GET", "/bkt?acl", "/bkt/key/?acl", b"").await;
    assert_eq!((r.status, r.code()), (403, "SignatureDoesNotMatch"), "{}", r.body);
    let date = http_date(SystemTime::now());
    let forged = format!("AWS fakes3:{}", base64_simd::STANDARD.encode_to_string(b"0123456789abcdef0123"));
    let r = send(
        a,
        req("GET", "/bkt")
            .query("acl")
            .anon()
            .header("authorization", &forged)
            .header("date", &date),
    )
    .await;
    assert_eq!((r.status, r.code()), (403, "SignatureDoesNotMatch"), "{}", r.body);

    // The body survives the second attempt: DeleteObjects removes the key.
    let delete = b"<Delete><Object><Key>k</Key></Object></Delete>";
    let r = v2(a, "POST", "/bkt?delete", "/bkt/?delete", delete).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.body.contains("<Deleted><Key>k</Key>"), "{}", r.body);
    assert_eq!(v2(a, "GET", "/bkt/k", "/bkt/k", b"").await.status, 404);

    // Presigned URLs.
    let expires = (SystemTime::now() + Duration::from_secs(600))
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    for resource in ["/bkt/", "/bkt"] {
        let sig = v2_signature(&format!("GET\n\n\n{expires}\n{resource}"));
        let sig = sig.replace('+', "%2B").replace('/', "%2F").replace('=', "%3D");
        let q = format!("AWSAccessKeyId=fakes3&Expires={expires}&Signature={sig}");
        let r = send(a, req("GET", "/bkt").query(&q).anon()).await;
        assert_eq!(r.status, 200, "{resource}: {}", r.body);
    }
    let q = format!("AWSAccessKeyId=fakes3&Expires={expires}&Signature=AAAAAAAAAAAAAAAAAAAAAAAAAAA%3D");
    let r = send(a, req("GET", "/bkt").query(&q).anon()).await;
    assert_eq!((r.status, r.code()), (403, "SignatureDoesNotMatch"), "{}", r.body);
    server.shutdown().await;
}

#[tokio::test]
async fn v2_bad_dates_are_access_denied() {
    let server = start(true).await;
    let a = server.addr();
    assert_eq!(v2(a, "PUT", "/bkt", "/bkt/", b"").await.status, 200);

    // (x-amz-date, status, code); signed with the header, which is how botocore's test hooks send it.
    let now = SystemTime::now();
    let cases: [(&str, u16, &str); 8] = [
        ("Bad Date", 403, "AccessDenied"),
        ("", 403, "AccessDenied"),
        ("Tue, 07 Jul 1950 21:53:04 GMT", 403, "AccessDenied"),
        ("Tue, 07 Jul 2010 21:53:04 GMT", 403, "RequestTimeTooSkewed"),
        ("Tue, 07 Jul 9999 21:53:04 GMT", 403, "RequestTimeTooSkewed"),
        ("20100707T215304Z", 403, "RequestTimeTooSkewed"),
        (&common::amz_date(now), 200, ""),
        // Not worked around: S3 accepts this well-formed date, s3s does not parse it.
        (&http_date(now), 400, "InvalidRequest"),
    ];
    for (i, (value, status, code)) in cases.into_iter().enumerate() {
        for (target, resource) in [
            (format!("/date-bkt-{i}"), format!("/date-bkt-{i}/")),
            (format!("/bkt/k{i}"), format!("/bkt/k{i}")),
        ] {
            let auth = v2_authorization("PUT", "", "", &[("x-amz-date", value)], &resource);
            let r = send(
                a,
                req("PUT", &target)
                    .anon()
                    .header("authorization", &auth)
                    .header("x-amz-date", value),
            )
            .await;
            assert_eq!(r.status, status, "{value:?} {target}: {}", r.body);
            assert_eq!(r.code(), code, "{value:?} {target}: {}", r.body);
        }
    }

    // No date at all.
    let auth = v2_authorization("PUT", "", "", &[], "/bkt/k");
    let r = send(a, req("PUT", "/bkt/k").anon().header("authorization", &auth).body(b"v")).await;
    assert_eq!((r.status, r.code()), (403, "AccessDenied"), "{}", r.body);

    // A stale Date header.
    let stale = "Tue, 07 Jul 2010 21:53:04 GMT";
    let auth = v2_authorization("PUT", "", stale, &[], "/bkt/k");
    let r = send(
        a,
        req("PUT", "/bkt/k")
            .anon()
            .header("authorization", &auth)
            .header("date", stale)
            .body(b"v"),
    )
    .await;
    assert_eq!((r.status, r.code()), (403, "RequestTimeTooSkewed"), "{}", r.body);
    server.shutdown().await;
}

/// A form with Python-requests-style parts: every field carries a filename.
fn multipart(boundary: &str, fields: &[(&str, &str)], file: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for (k, v) in fields {
        out.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"; filename=\"{k}\"\r\n\r\n{v}\r\n").as_bytes(),
        );
    }
    out.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"f.txt\"\r\n\r\n").as_bytes());
    out.extend_from_slice(file);
    out.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    out
}

/// POSTs a SigV2-signed form whose policy has `conditions` (JSON array items) and whose fields are
/// `key` and `acl`.
async fn post_form(addr: SocketAddr, conditions: &str) -> Resp {
    post_form_signed(addr, conditions, true).await
}

async fn post_form_signed(addr: SocketAddr, conditions: &str, signed: bool) -> Resp {
    let expires = common::amz_date(SystemTime::now() + Duration::from_secs(600));
    let expires = format!(
        "{}-{}-{}T{}:{}:{}Z",
        &expires[0..4],
        &expires[4..6],
        &expires[6..8],
        &expires[9..11],
        &expires[11..13],
        &expires[13..15]
    );
    let policy = format!(r#"{{"expiration":"{expires}","conditions":[{conditions}]}}"#);
    let policy = base64_simd::STANDARD.encode_to_string(policy.as_bytes());
    let signature = v2_signature(&policy);
    let mut fields = vec![
        ("key", "foo.txt"),
        ("acl", "private"),
        ("AWSAccessKeyId", "fakes3"),
        ("policy", policy.as_str()),
    ];
    if signed {
        fields.push(("signature", signature.as_str()));
    }
    let body = multipart("XBOUNDARYX", &fields, b"bar");
    send(
        addr,
        req("POST", "/bkt")
            .anon()
            .header("content-type", "multipart/form-data; boundary=XBOUNDARYX")
            .body(&body),
    )
    .await
}

#[tokio::test]
async fn post_policy_violations_are_access_denied() {
    for auth in [true, false] {
        let server = start(auth).await;
        let a = server.addr();
        assert_eq!(send(a, req("PUT", "/bkt")).await.status, 200);
        let ok = r#"{"bucket":"bkt"},["starts-with","$key","foo"],{"acl":"private"}"#;
        let r = post_form(a, ok).await;
        assert_eq!(r.status, 204, "auth={auth}: {}", r.body);

        for (what, conditions) in [
            (
                "wrong bucket",
                r#"{"bucket":"other"},["starts-with","$key","foo"],{"acl":"private"}"#,
            ),
            ("eq", r#"{"bucket":"bkt"},["starts-with","$key","foo"],{"acl":"public-read"}"#),
            ("starts-with", r#"{"bucket":"bkt"},["starts-with","$key","bar"],{"acl":"private"}"#),
            ("field not covered", r#"{"bucket":"bkt"},["starts-with","$key","foo"]"#),
            (
                "condition field missing",
                r#"{"bucket":"bkt"},["starts-with","$key","foo"],{"acl":"private"},["eq","$x-amz-meta-foo","bar"]"#,
            ),
        ] {
            let r = post_form(a, conditions).await;
            assert_eq!((r.status, r.code()), (403, "AccessDenied"), "auth={auth} {what}: {}", r.body);
            assert!(r.body.contains("<RequestId>"), "{}", r.body);
        }

        // A form without a signature is refused for that (s3s judges its policy first, still 400).
        let r = post_form_signed(a, r#"{"bucket":"bkt"},["starts-with","$key","bar"],{"acl":"private"}"#, false).await;
        assert_eq!(r.status, 400, "auth={auth}: {}", r.body);

        // A size range is not a policy violation, and a policy nobody can read is still malformed.
        let r = post_form(a, &format!(r#"{ok},["content-length-range",0,1]"#)).await;
        assert_eq!((r.status, r.code()), (400, "EntityTooLarge"), "auth={auth}: {}", r.body);
        let garbage = base64_simd::STANDARD.encode_to_string(b"not json");
        let body = multipart(
            "XBOUNDARYX",
            &[
                ("key", "foo.txt"),
                ("AWSAccessKeyId", "fakes3"),
                ("policy", &garbage),
                ("signature", &v2_signature(&garbage)),
            ],
            b"bar",
        );
        let r = send(
            a,
            req("POST", "/bkt")
                .anon()
                .header("content-type", "multipart/form-data; boundary=XBOUNDARYX")
                .body(&body),
        )
        .await;
        assert_eq!((r.status, r.code()), (400, "InvalidPolicyDocument"), "auth={auth}: {}", r.body);
        server.shutdown().await;
    }
}

#[tokio::test]
async fn uploads_without_content_length() {
    for auth in [true, false] {
        let server = start(auth).await;
        let a = server.addr();
        assert_eq!(send(a, req("PUT", "/bkt")).await.status, 200);
        let r = send(a, req("PUT", "/bkt/chunked").chunked().body(b"hello chunked world")).await;
        assert_eq!(r.status, 200, "auth={auth}: {}", r.body);
        let r = send(a, req("GET", "/bkt/chunked")).await;
        assert_eq!((r.status, r.body.as_str()), (200, "hello chunked world"), "auth={auth}");

        // An empty body works too.
        let r = send(a, req("PUT", "/bkt/empty").chunked()).await;
        assert_eq!(r.status, 200, "auth={auth}: {}", r.body);
        assert_eq!(send(a, req("GET", "/bkt/empty")).await.body, "");
        server.shutdown().await;
    }
}

#[tokio::test]
async fn sigv4_takes_the_time_from_date() {
    let server = start(true).await;
    let a = server.addr();
    assert_eq!(send(a, req("PUT", "/bkt")).await.status, 200);
    let r = send(a, req("PUT", "/bkt/dated").date_header().body(b"dated")).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(send(a, req("GET", "/bkt/dated").date_header()).await.body, "dated");
    server.shutdown().await;
}
