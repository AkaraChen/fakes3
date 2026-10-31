//! Small AWS-behaviour fixes found by running ceph/s3-tests (conformance/S3-TESTS.md), each pinned
//! with the request that exposed it. Real sockets, SigV4 auth on.

mod common;

use std::net::SocketAddr;

use common::{Resp, req, send};
use fakes3::{Config, RequestFilter, Server};

async fn start() -> (Server, SocketAddr) {
    let server = Server::start(Config::default()).await.unwrap();
    let addr = server.addr();
    (server, addr)
}

async fn make_bucket(a: SocketAddr, name: &str) {
    let r = send(a, req("PUT", &format!("/{name}"))).await;
    assert_eq!(r.status, 200, "{}", r.body);
}

async fn put(a: SocketAddr, path: &str, body: &[u8]) -> Resp {
    send(a, req("PUT", path).body(body)).await
}

/// Text of the first `<name>` element.
fn tag<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let start = xml.find(&format!("<{name}>"))? + name.len() + 2;
    let end = xml[start..].find(&format!("</{name}>"))?;
    Some(&xml[start..start + end])
}

const VERSIONING_ENABLED: &str = "<VersioningConfiguration><Status>Enabled</Status></VersioningConfiguration>";
const SSE_C_KEY: &str = "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=";
const SSE_C_MD5: &str = "hRasmdxgYDKV3nvbahU1MA==";
const OTHER_KEY: &str = "ZmVkY2JhOTg3NjU0MzIxMGZlZGNiYTk4NzY1NDMyMTA=";
const OTHER_MD5: &str = "dT1y7SEiqn6YCJ3QeqwoIw==";

#[tokio::test]
async fn every_response_carries_a_request_id_that_the_log_and_error_body_repeat() {
    let (s, a) = start().await;
    make_bucket(a, "bkt").await;
    let ok = put(a, "/bkt/k", b"x").await;
    let id = ok.header("x-amz-request-id").unwrap().to_owned();
    assert_eq!(id.len(), 16);
    assert!(id.bytes().all(|c| c.is_ascii_digit() || (b'A'..=b'F').contains(&c)), "{id}");
    assert_eq!(ok.header("x-amz-id-2").unwrap().len(), 64);

    // An error from a handler (NoSuchKey) and one from s3s itself (bad signature) both say it.
    let err = send(a, req("GET", "/bkt/missing")).await;
    assert_eq!(err.code(), "NoSuchKey");
    let eid = err.header("x-amz-request-id").unwrap();
    assert_eq!(tag(&err.body, "RequestId"), Some(eid));
    assert_eq!(tag(&err.body, "HostId"), err.header("x-amz-id-2"));
    let anon = send(a, req("GET", "/bkt/k").anon()).await;
    assert_eq!(anon.status, 403);
    assert_eq!(tag(&anon.body, "RequestId"), anon.header("x-amz-request-id"));
    assert_ne!(eid, anon.header("x-amz-request-id").unwrap());

    // The request log records the id the client saw.
    let (rows, _) = s.requests(&RequestFilter::default());
    let logged: Vec<_> = rows.iter().map(|r| r.request_id()).collect();
    assert!(logged.contains(&id) && logged.iter().any(|l| l == eid), "{logged:?}");
}

#[tokio::test]
async fn creating_an_owned_bucket_again_succeeds_and_leaves_it_alone() {
    let (_s, a) = start().await;
    make_bucket(a, "bkt").await;
    put(a, "/bkt/k", b"x").await;
    let r = send(a, req("PUT", "/bkt").header("x-amz-acl", "public-read")).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(send(a, req("GET", "/bkt/k")).await.body, "x");
    // The second request did not reset the ACL.
    let acl = send(a, req("GET", "/bkt").query("acl")).await;
    assert!(!acl.body.contains("AllUsers"), "{}", acl.body);
}

#[tokio::test]
async fn get_with_part_number_reports_the_part_checksum_and_type() {
    let (_s, a) = start().await;
    make_bucket(a, "bkt").await;
    let up = send(
        a,
        req("POST", "/bkt/mp").query("uploads").header("x-amz-checksum-algorithm", "CRC32"),
    )
    .await;
    let uid = tag(&up.body, "UploadId").unwrap().to_owned();
    let big = vec![b'A'; 5 * 1024 * 1024];
    let q1 = format!("partNumber=1&uploadId={uid}");
    let p1 = send(
        a,
        req("PUT", "/bkt/mp")
            .query(&q1)
            .header("x-amz-checksum-algorithm", "CRC32")
            .body(&big),
    )
    .await;
    let q2 = format!("partNumber=2&uploadId={uid}");
    let p2 = send(
        a,
        req("PUT", "/bkt/mp")
            .query(&q2)
            .header("x-amz-checksum-algorithm", "CRC32")
            .body(b"tail"),
    )
    .await;
    let (c1, c2) = (
        p1.header("x-amz-checksum-crc32").unwrap(),
        p2.header("x-amz-checksum-crc32").unwrap(),
    );
    assert_ne!(c1, c2);
    let body = format!(
        "<CompleteMultipartUpload><Part><ETag>{}</ETag><ChecksumCRC32>{c1}</ChecksumCRC32><PartNumber>1</PartNumber></Part>\
         <Part><ETag>{}</ETag><ChecksumCRC32>{c2}</ChecksumCRC32><PartNumber>2</PartNumber></Part></CompleteMultipartUpload>",
        p1.header("etag").unwrap(),
        p2.header("etag").unwrap()
    );
    let done = send(a, req("POST", "/bkt/mp").query(&format!("uploadId={uid}")).body(body.as_bytes())).await;
    assert_eq!(done.status, 200, "{}", done.body);

    for (n, want) in [("1", c1), ("2", c2)] {
        let g = send(
            a,
            req("GET", "/bkt/mp")
                .query(&format!("partNumber={n}"))
                .header("x-amz-checksum-mode", "ENABLED"),
        )
        .await;
        assert_eq!(g.status, 206);
        assert_eq!(g.header("x-amz-checksum-type"), Some("COMPOSITE"));
        assert_eq!(g.header("x-amz-checksum-crc32"), Some(want));
    }
    // Without the mode header nothing is reported, as before.
    let g = send(a, req("GET", "/bkt/mp").query("partNumber=2")).await;
    assert!(g.header("x-amz-checksum-type").is_none());
}

#[tokio::test]
async fn sse_c_part_with_a_different_key_is_a_bad_request() {
    let (_s, a) = start().await;
    make_bucket(a, "bkt").await;
    let alg = ("x-amz-server-side-encryption-customer-algorithm", "AES256");
    let up = send(
        a,
        req("POST", "/bkt/mp")
            .query("uploads")
            .header(alg.0, alg.1)
            .header("x-amz-server-side-encryption-customer-key", SSE_C_KEY)
            .header("x-amz-server-side-encryption-customer-key-md5", SSE_C_MD5),
    )
    .await;
    let uid = tag(&up.body, "UploadId").unwrap().to_owned();
    let q = format!("partNumber=1&uploadId={uid}");
    let part = |key: &'static str, md5: &'static str| {
        send(
            a,
            req("PUT", "/bkt/mp")
                .query(&q)
                .header(alg.0, alg.1)
                .header("x-amz-server-side-encryption-customer-key", key)
                .header("x-amz-server-side-encryption-customer-key-md5", md5)
                .body(b"part"),
        )
    };
    let bad = part(OTHER_KEY, OTHER_MD5).await;
    assert_eq!((bad.status, bad.code()), (400, "InvalidRequest"), "{}", bad.body);
    assert_eq!(part(SSE_C_KEY, SSE_C_MD5).await.status, 200);
}

#[tokio::test]
async fn list_objects_echo_marker_and_owner_and_omit_an_empty_delimiter() {
    let (_s, a) = start().await;
    make_bucket(a, "bkt").await;
    put(a, "/bkt/k", b"x").await;
    let v1 = send(a, req("GET", "/bkt").query("delimiter=")).await;
    assert!(
        v1.body.contains("<Marker></Marker>") || v1.body.contains("<Marker/>"),
        "{}",
        v1.body
    );
    assert!(
        v1.body.contains("<Owner><DisplayName>fakes3</DisplayName><ID>fakes3</ID></Owner>"),
        "{}",
        v1.body
    );
    assert!(!v1.body.contains("<Delimiter"), "{}", v1.body);

    let v2 = send(a, req("GET", "/bkt").query("list-type=2&delimiter=")).await;
    assert!(!v2.body.contains("<Owner>") && !v2.body.contains("<Delimiter"), "{}", v2.body);
    let v2 = send(a, req("GET", "/bkt").query("fetch-owner=true&list-type=2")).await;
    assert!(v2.body.contains("<Owner>"), "{}", v2.body);
}

#[tokio::test]
async fn encoding_type_url_percent_encodes_spaces() {
    let (_s, a) = start().await;
    make_bucket(a, "bkt").await;
    put(a, "/bkt/quux%20ab/thud", b"x").await;
    put(a, "/bkt/asdf%2Bb", b"x").await;
    put(a, "/bkt/foo+1/bar", b"x").await;
    let r = send(a, req("GET", "/bkt").query("delimiter=%2F&encoding-type=url")).await;
    assert!(r.body.contains("<Prefix>quux%20ab/</Prefix>"), "{}", r.body);
    assert!(
        r.body.contains("<Key>asdf%2Bb</Key>") && r.body.contains("<Prefix>foo%2B1/</Prefix>"),
        "{}",
        r.body
    );
    let r = send(a, req("GET", "/bkt").query("encoding-type=url&list-type=2&prefix=quux%20")).await;
    assert!(
        r.body.contains("<Prefix>quux%20</Prefix>") && r.body.contains("<Key>quux%20ab/thud</Key>"),
        "{}",
        r.body
    );
}

#[tokio::test]
async fn object_lock_mode_change_needs_bypass_and_default_period_must_be_positive() {
    let (_s, a) = start().await;
    let r = send(a, req("PUT", "/lock").header("x-amz-bucket-object-lock-enabled", "true")).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let until = "2099-01-01T00:00:00Z";
    let put_obj = send(
        a,
        req("PUT", "/lock/k")
            .header("x-amz-object-lock-mode", "GOVERNANCE")
            .header("x-amz-object-lock-retain-until-date", until)
            .body(b"x"),
    )
    .await;
    assert_eq!(put_obj.status, 200, "{}", put_obj.body);
    let to_compliance = format!("<Retention><Mode>COMPLIANCE</Mode><RetainUntilDate>{until}</RetainUntilDate></Retention>");
    let r = send(a, req("PUT", "/lock/k").query("retention").body(to_compliance.as_bytes())).await;
    assert_eq!((r.status, r.code()), (403, "AccessDenied"), "{}", r.body);
    let r = send(
        a,
        req("PUT", "/lock/k")
            .query("retention")
            .header("x-amz-bypass-governance-retention", "true")
            .body(to_compliance.as_bytes()),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);

    for period in ["<Days>0</Days>", "<Years>-1</Years>"] {
        let cfg = format!(
            "<ObjectLockConfiguration><ObjectLockEnabled>Enabled</ObjectLockEnabled><Rule><DefaultRetention><Mode>GOVERNANCE</Mode>{period}</DefaultRetention></Rule></ObjectLockConfiguration>"
        );
        let r = send(a, req("PUT", "/lock").query("object-lock").body(cfg.as_bytes())).await;
        assert_eq!((r.status, r.code()), (400, "InvalidRetentionPeriod"), "{period}: {}", r.body);
    }
}

#[tokio::test]
async fn content_md5_must_be_16_bytes_to_be_a_digest_at_all() {
    let (_s, a) = start().await;
    make_bucket(a, "bkt").await;
    for md5 in ["", "YWJj"] {
        let r = send(a, req("PUT", "/bkt/k").header("content-md5", md5).body(b"x")).await;
        assert_eq!((r.status, r.code()), (400, "InvalidDigest"), "{md5:?}: {}", r.body);
    }
    // 16 bytes of base64, but not this body's digest.
    let r = send(a, req("PUT", "/bkt/k").header("content-md5", "YWJjZGVmZ2hpamtsbW5vcA==").body(b"x")).await;
    assert_eq!((r.status, r.code()), (400, "BadDigest"), "{}", r.body);
    // x's real digest.
    let r = send(a, req("PUT", "/bkt/k").header("content-md5", "ndTkYSaMgDT1yFZOFVxnpg==").body(b"x")).await;
    assert_eq!(r.status, 200, "{}", r.body);
}

#[tokio::test]
async fn delete_objects_takes_at_most_a_thousand_keys() {
    let (_s, a) = start().await;
    make_bucket(a, "bkt").await;
    let keys = |n: usize| {
        let objs: String = (0..n).map(|i| format!("<Object><Key>k{i}</Key></Object>")).collect();
        format!("<Delete>{objs}</Delete>")
    };
    let digest = |body: &str| {
        use sha2::{Digest, Sha256};
        base64_simd::STANDARD.encode_to_string(Sha256::digest(body.as_bytes()))
    };
    let too_many = keys(1001);
    let r = send(
        a,
        req("POST", "/bkt")
            .query("delete")
            .header("x-amz-checksum-sha256", &digest(&too_many))
            .body(too_many.as_bytes()),
    )
    .await;
    assert_eq!((r.status, r.code()), (400, "MalformedXML"), "{}", r.body);
    let ok = keys(1000);
    let r = send(
        a,
        req("POST", "/bkt")
            .query("delete")
            .header("x-amz-checksum-sha256", &digest(&ok))
            .body(ok.as_bytes()),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
}

#[tokio::test]
async fn lifecycle_rules_without_an_id_get_one() {
    let (_s, a) = start().await;
    make_bucket(a, "bkt").await;
    let cfg = "<LifecycleConfiguration><Rule><Filter><Prefix>test1/</Prefix></Filter><Status>Enabled</Status><Expiration><Days>31</Days></Expiration></Rule>\
               <Rule><ID>mine</ID><Filter><Prefix>test2/</Prefix></Filter><Status>Enabled</Status><Expiration><Days>120</Days></Expiration></Rule></LifecycleConfiguration>";
    let r = send(a, req("PUT", "/bkt").query("lifecycle").body(cfg.as_bytes())).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let g = send(a, req("GET", "/bkt").query("lifecycle")).await;
    assert_eq!(g.body.matches("<ID>").count(), 2, "{}", g.body);
    assert!(g.body.contains("<ID>mine</ID>"));
}

#[tokio::test]
async fn sse_headers_on_get_and_head_are_a_bad_request() {
    let (_s, a) = start().await;
    make_bucket(a, "bkt").await;
    put(a, "/bkt/k", b"x").await;
    for header in [
        ("x-amz-server-side-encryption", "AES256"),
        ("x-amz-server-side-encryption-aws-kms-key-id", "testkey-1"),
    ] {
        let g = send(a, req("GET", "/bkt/k").header(header.0, header.1)).await;
        assert_eq!((g.status, g.code()), (400, "InvalidRequest"), "{}", g.body);
        assert_eq!(send(a, req("HEAD", "/bkt/k").header(header.0, header.1)).await.status, 400);
    }
    assert_eq!(send(a, req("GET", "/bkt/k")).await.status, 200);
}

#[tokio::test]
async fn only_enabled_versioning_reports_a_version_id_on_writes() {
    let (_s, a) = start().await;
    make_bucket(a, "bkt").await;
    assert!(put(a, "/bkt/k", b"x").await.header("x-amz-version-id").is_none());
    let r = send(a, req("PUT", "/bkt").query("versioning").body(VERSIONING_ENABLED.as_bytes())).await;
    assert_eq!(r.status, 200);
    assert!(put(a, "/bkt/k", b"x").await.header("x-amz-version-id").is_some_and(|v| v != "null"));
    let suspended = "<VersioningConfiguration><Status>Suspended</Status></VersioningConfiguration>";
    send(a, req("PUT", "/bkt").query("versioning").body(suspended.as_bytes())).await;
    let r = put(a, "/bkt/k", b"x").await;
    assert_eq!(r.status, 200);
    assert!(r.header("x-amz-version-id").is_none());
}

#[tokio::test]
async fn mfa_delete_disabled_needs_no_token_while_it_is_off() {
    let (_s, a) = start().await;
    make_bucket(a, "bkt").await;
    let cfg = "<VersioningConfiguration><Status>Enabled</Status><MfaDelete>Disabled</MfaDelete></VersioningConfiguration>";
    let r = send(a, req("PUT", "/bkt").query("versioning").body(cfg.as_bytes())).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let on = "<VersioningConfiguration><Status>Enabled</Status><MfaDelete>Enabled</MfaDelete></VersioningConfiguration>";
    let r = send(a, req("PUT", "/bkt").query("versioning").body(on.as_bytes())).await;
    assert_eq!((r.status, r.code()), (403, "AccessDenied"));
}

#[tokio::test]
async fn reads_default_the_content_type() {
    let (_s, a) = start().await;
    make_bucket(a, "bkt").await;
    // The test client sends no Content-Type.
    put(a, "/bkt/k", b"x").await;
    assert_eq!(
        send(a, req("GET", "/bkt/k")).await.header("content-type"),
        Some("binary/octet-stream")
    );
    assert_eq!(
        send(a, req("HEAD", "/bkt/k")).await.header("content-type"),
        Some("binary/octet-stream")
    );
    put_typed(a, "/bkt/t", "text/plain").await;
    assert_eq!(send(a, req("GET", "/bkt/t")).await.header("content-type"), Some("text/plain"));
}

async fn put_typed(a: SocketAddr, path: &str, ctype: &str) -> Resp {
    send(a, req("PUT", path).header("content-type", ctype).body(b"x")).await
}

#[tokio::test]
async fn tagging_limits_and_header_tag_order() {
    let (_s, a) = start().await;
    make_bucket(a, "bkt").await;
    put(a, "/bkt/k", b"x").await;
    let tags: String = (0..11).map(|i| format!("<Tag><Key>k{i}</Key><Value>v</Value></Tag>")).collect();
    let body = format!("<Tagging><TagSet>{tags}</TagSet></Tagging>");
    let r = send(a, req("PUT", "/bkt/k").query("tagging").body(body.as_bytes())).await;
    assert_eq!((r.status, r.code()), (400, "InvalidTag"), "{}", r.body);

    send(a, req("PUT", "/bkt/h").header("x-amz-tagging", "foo=bar&bar").body(b"x")).await;
    let g = send(a, req("GET", "/bkt/h").query("tagging")).await;
    let (bar, foo) = (g.body.find("<Key>bar</Key>").unwrap(), g.body.find("<Key>foo</Key>").unwrap());
    assert!(bar < foo, "{}", g.body);
}

#[tokio::test]
async fn complete_multipart_checks_the_request_level_checksum() {
    let (_s, a) = start().await;
    make_bucket(a, "bkt").await;
    let up = send(
        a,
        req("POST", "/bkt/mp").query("uploads").header("x-amz-checksum-algorithm", "SHA256"),
    )
    .await;
    let uid = tag(&up.body, "UploadId").unwrap().to_owned();
    let q = format!("partNumber=1&uploadId={uid}");
    let p = send(
        a,
        req("PUT", "/bkt/mp")
            .query(&q)
            .header("x-amz-checksum-algorithm", "SHA256")
            .body(b"only part"),
    )
    .await;
    let sum = p.header("x-amz-checksum-sha256").unwrap();
    let body = format!(
        "<CompleteMultipartUpload><Part><ETag>{}</ETag><ChecksumSHA256>{sum}</ChecksumSHA256><PartNumber>1</PartNumber></Part></CompleteMultipartUpload>",
        p.header("etag").unwrap()
    );
    let complete = |checksum: String| {
        let (q, body) = (format!("uploadId={uid}"), body.clone());
        async move {
            send(
                a,
                req("POST", "/bkt/mp")
                    .query(&q)
                    .header("x-amz-checksum-sha256", &checksum)
                    .body(body.as_bytes()),
            )
            .await
        }
    };
    let r = complete("bad".into()).await;
    assert_eq!((r.status, r.code()), (400, "BadDigest"), "{}", r.body);
    // Well-formed but wrong, too.
    let r = complete("Ok6Cs5b96ux6+MWQkJO7UBT5sKPBeXBLwvj/hK89smg=-1".into()).await;
    assert_eq!((r.status, r.code()), (400, "BadDigest"), "{}", r.body);
    // The upload is still there; the composite value of its one part completes it.
    let composite = {
        use sha2::{Digest, Sha256};
        let raw = base64_simd::STANDARD.decode_to_vec(sum).unwrap();
        format!("{}-1", base64_simd::STANDARD.encode_to_string(Sha256::digest(raw)))
    };
    let r = complete(composite).await;
    assert_eq!(r.status, 200, "{}", r.body);
}

#[tokio::test]
async fn a_foreign_expected_bucket_owner_is_denied() {
    let (_s, a) = start().await;
    make_bucket(a, "bkt").await;
    let r = send(a, req("GET", "/bkt").header("x-amz-expected-bucket-owner", "111122223333")).await;
    assert_eq!((r.status, r.code()), (403, "AccessDenied"), "{}", r.body);
    let r = send(
        a,
        req("PUT", "/bkt/k")
            .header("x-amz-expected-bucket-owner", "111122223333")
            .body(b"x"),
    )
    .await;
    assert_eq!(r.status, 403);
    assert_eq!(send(a, req("GET", "/bkt/k")).await.status, 404, "nothing was written");
    let r = send(a, req("GET", "/bkt").header("x-amz-expected-bucket-owner", "000000000000")).await;
    assert_eq!(r.status, 200, "{}", r.body);
}
