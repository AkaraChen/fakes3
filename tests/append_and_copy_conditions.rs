//! Appends (PutObject with `x-amz-write-offset-bytes`) and CopyObject's conditions on the
//! destination, over real HTTP.

use std::net::SocketAddr;

use common::{Resp, req, send};
use fakes3::{Config, Server};

mod common;

const SSE_C_KEY: &str = "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY="; // "0123456789abcdef0123456789abcdef"
const SSE_C_MD5: &str = "hRasmdxgYDKV3nvbahU1MA==";

async fn start() -> Server {
    let s = Server::start(Config::default()).await.unwrap();
    assert_eq!(send(s.addr(), req("PUT", "/bkt")).await.status, 200);
    s
}

async fn put(addr: SocketAddr, path: &str, headers: &[(&str, &str)], body: &[u8]) -> Resp {
    let mut r = req("PUT", path).body(body);
    for (k, v) in headers {
        r = r.header(k, v);
    }
    send(addr, r).await
}

async fn append(addr: SocketAddr, path: &str, offset: usize, body: &[u8]) -> Resp {
    put(addr, path, &[("x-amz-write-offset-bytes", &offset.to_string())], body).await
}

async fn get(addr: SocketAddr, path: &str, query: &str) -> Resp {
    send(addr, req("GET", path).query(query)).await
}

async fn crc64nvme(addr: SocketAddr, path: &str) -> Option<String> {
    let r = send(addr, req("HEAD", path).header("x-amz-checksum-mode", "ENABLED")).await;
    r.header("x-amz-checksum-crc64nvme").map(str::to_owned)
}

/// CopyObject from `/bkt/src` with one conditional header.
async fn copy(addr: SocketAddr, to: &str, condition: &str, value: &str) -> Resp {
    send(
        addr,
        req("PUT", to).header("x-amz-copy-source", "/bkt/src").header(condition, value),
    )
    .await
}

#[tokio::test]
async fn append_extends_the_object() {
    let s = start().await;
    let a = s.addr();
    assert_eq!(put(a, "/bkt/log", &[("content-type", "text/plain")], b"abc").await.status, 200);

    let r = append(a, "/bkt/log", 3, b"def").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.header("x-amz-object-size"), Some("6"));
    let etag = r.header("etag").unwrap().to_owned();
    assert!(etag.ends_with("-2\""), "{etag}");

    let g = get(a, "/bkt/log", "").await;
    assert_eq!(g.body, "abcdef");
    assert_eq!(g.header("etag"), Some(etag.as_str()));
    // The first write's metadata stays; each append is a part.
    assert_eq!(g.header("content-type"), Some("text/plain"));
    assert_eq!(get(a, "/bkt/log", "partNumber=2").await.body, "def");

    // The full-object CRC64NVME covers both writes: the same as one PUT of the whole content.
    put(a, "/bkt/whole", &[], b"abcdef").await;
    assert!(crc64nvme(a, "/bkt/log").await.is_some());
    assert_eq!(crc64nvme(a, "/bkt/log").await, crc64nvme(a, "/bkt/whole").await);
    s.shutdown().await;
}

#[tokio::test]
async fn append_needs_the_current_size_as_offset() {
    let s = start().await;
    let a = s.addr();
    // Offset 0 creates a missing object; any other offset is wrong for it.
    let r = append(a, "/bkt/new", 1, b"x").await;
    assert_eq!((r.status, r.code()), (400, "InvalidWriteOffset"));
    assert_eq!(append(a, "/bkt/new", 0, b"abc").await.status, 200);

    for wrong in [0, 2, 4] {
        let r = append(a, "/bkt/new", wrong, b"x").await;
        assert_eq!((r.status, r.code()), (400, "InvalidWriteOffset"), "offset {wrong}");
    }
    assert_eq!(get(a, "/bkt/new", "").await.body, "abc");
    s.shutdown().await;
}

#[tokio::test]
async fn append_in_a_versioned_bucket_adds_a_version() {
    let s = start().await;
    let a = s.addr();
    let body = b"<VersioningConfiguration><Status>Enabled</Status></VersioningConfiguration>";
    assert_eq!(send(a, req("PUT", "/bkt").query("versioning").body(body)).await.status, 200);
    let first = put(a, "/bkt/k", &[], b"one").await;
    let first = first.header("x-amz-version-id").unwrap().to_owned();
    let r = append(a, "/bkt/k", 3, b"two").await;
    let second = r.header("x-amz-version-id").unwrap();
    assert_ne!(second, first);
    assert_eq!(get(a, "/bkt/k", "").await.body, "onetwo");
    assert_eq!(get(a, "/bkt/k", &format!("versionId={first}")).await.body, "one");
    s.shutdown().await;
}

#[tokio::test]
async fn append_keeps_the_encryption_of_the_object() {
    let s = start().await;
    let a = s.addr();
    let key = [
        ("x-amz-server-side-encryption-customer-algorithm", "AES256"),
        ("x-amz-server-side-encryption-customer-key", SSE_C_KEY),
        ("x-amz-server-side-encryption-customer-key-md5", SSE_C_MD5),
    ];
    assert_eq!(put(a, "/bkt/k", &key, b"abc").await.status, 200);
    let r = append(a, "/bkt/k", 3, b"def").await;
    assert_eq!((r.status, r.code()), (400, "EncryptionTypeMismatch"));
    let mut with_key = key.to_vec();
    with_key.push(("x-amz-write-offset-bytes", "3"));
    assert_eq!(put(a, "/bkt/k", &with_key, b"def").await.status, 200);
    let mut g = req("GET", "/bkt/k");
    for (k, v) in key {
        g = g.header(k, v);
    }
    assert_eq!(send(a, g).await.body, "abcdef");
    s.shutdown().await;
}

#[tokio::test]
async fn copy_honours_conditions_on_the_destination() {
    let s = start().await;
    let a = s.addr();
    put(a, "/bkt/src", &[], b"source").await;
    let dst = put(a, "/bkt/dst", &[], b"destination").await;
    let dst_etag = dst.header("etag").unwrap().to_owned();

    // If-None-Match: * refuses an existing destination and allows a new one.
    let r = copy(a, "/bkt/dst", "if-none-match", "*").await;
    assert_eq!((r.status, r.code()), (412, "PreconditionFailed"));
    assert_eq!(copy(a, "/bkt/fresh", "if-none-match", "*").await.status, 200);

    // If-Match compares with the destination's ETag, not the source's.
    let r = copy(a, "/bkt/dst", "if-match", "\"0123456789abcdef0123456789abcdef\"").await;
    assert_eq!((r.status, r.code()), (412, "PreconditionFailed"));
    let r = copy(a, "/bkt/missing", "if-match", &dst_etag).await;
    assert_eq!((r.status, r.code()), (404, "NoSuchKey"));
    assert_eq!(get(a, "/bkt/dst", "").await.body, "destination");
    let r = copy(a, "/bkt/dst", "if-match", &dst_etag).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(get(a, "/bkt/dst", "").await.body, "source");
    s.shutdown().await;
}
