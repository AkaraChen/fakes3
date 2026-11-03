//! The `response-*` overrides of GetObject and HeadObject, and `<ETag>` conditions in
//! DeleteObjects, over real HTTP.

use std::net::SocketAddr;

use common::{Resp, req, send};
use fakes3::{Config, Server};

mod common;

async fn start() -> Server {
    let s = Server::start(Config::default()).await.unwrap();
    assert_eq!(send(s.addr(), req("PUT", "/bkt")).await.status, 200);
    s
}

async fn put(addr: SocketAddr, key: &str, body: &str) -> String {
    let r = send(addr, req("PUT", &format!("/bkt/{key}")).body(body.as_bytes())).await;
    assert_eq!(r.status, 200, "{}", r.body);
    r.header("etag").unwrap().to_owned()
}

async fn delete_objects(addr: SocketAddr, objects: &[(&str, Option<&str>)]) -> Resp {
    use sha2::{Digest, Sha256};
    let objects: String = objects
        .iter()
        .map(|(key, etag)| match etag {
            Some(e) => format!("<Object><Key>{key}</Key><ETag>{e}</ETag></Object>"),
            None => format!("<Object><Key>{key}</Key></Object>"),
        })
        .collect();
    let body = format!("<Delete>{objects}</Delete>");
    let sum = base64_simd::STANDARD.encode_to_string(Sha256::digest(body.as_bytes()));
    let r = send(
        addr,
        req("POST", "/bkt")
            .query("delete")
            .header("x-amz-checksum-sha256", &sum)
            .body(body.as_bytes()),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    r
}

async fn exists(addr: SocketAddr, key: &str) -> bool {
    send(addr, req("HEAD", &format!("/bkt/{key}"))).await.status == 200
}

const OVERRIDES: &str = "response-cache-control=no-store&response-content-disposition=attachment&\
response-content-encoding=identity&response-content-language=fr&response-content-type=text%2Fx-test&\
response-expires=Thu%2C%2001%20Jan%202099%2000%3A00%3A00%20GMT";

#[tokio::test]
async fn head_and_get_apply_response_overrides() {
    let s = start().await;
    let a = s.addr();
    put(a, "k", "body").await;
    for method in ["GET", "HEAD"] {
        let plain = send(a, req(method, "/bkt/k")).await;
        assert_eq!(plain.header("content-type"), Some("binary/octet-stream"), "{method}");
        assert_eq!(plain.header("cache-control"), None, "{method}");

        let r = send(a, req(method, "/bkt/k").query(OVERRIDES)).await;
        assert_eq!(r.status, 200, "{method}: {}", r.body);
        let headers = [
            ("cache-control", "no-store"),
            ("content-disposition", "attachment"),
            ("content-encoding", "identity"),
            ("content-language", "fr"),
            ("content-type", "text/x-test"),
            ("expires", "Thu, 01 Jan 2099 00:00:00 GMT"),
        ];
        for (name, want) in headers {
            assert_eq!(r.header(name), Some(want), "{method} {name}");
        }
    }
    s.shutdown().await;
}

#[tokio::test]
async fn delete_objects_checks_each_etag() {
    let s = start().await;
    let a = s.addr();
    let current = put(a, "current", "a").await;
    let stale = put(a, "stale", "b").await;
    put(a, "stale", "replaced").await;
    put(a, "plain", "c").await;

    let r = delete_objects(
        a,
        &[
            ("current", Some(&current)),
            ("stale", Some(&stale)),
            ("missing", Some(&current)),
            ("plain", None),
        ],
    )
    .await;
    // A failed condition fails only its own entry.
    assert!(r.body.contains("<Code>PreconditionFailed</Code><Key>stale</Key>"), "{}", r.body);
    assert!(r.body.contains("<Code>NoSuchKey</Code><Key>missing</Key>"), "{}", r.body);
    assert!(r.body.contains("<Deleted><Key>current</Key></Deleted>"), "{}", r.body);
    assert!(r.body.contains("<Deleted><Key>plain</Key></Deleted>"), "{}", r.body);
    assert!(!exists(a, "current").await);
    assert!(!exists(a, "plain").await);
    assert!(exists(a, "stale").await);
    s.shutdown().await;
}
