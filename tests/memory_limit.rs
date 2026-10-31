//! Memory accounting and the memory limit, end to end over HTTP. Limits here are a few KiB.

use fakes3::{Config, MemoryLimit, Server};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

mod common;

const KIB: usize = 1024;

struct Resp {
    status: u16,
    body: String,
}

/// One request on a fresh connection. `chunked` sends the body without a Content-Length.
async fn send(server: &Server, method: &str, path: &str, headers: &[(&str, &str)], body: &[u8], chunked: bool) -> Resp {
    let mut s = TcpStream::connect(server.addr()).await.unwrap();
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nAuthorization: {}\r\n",
        common::UNVERIFIED_AUTH
    );
    for (k, v) in headers {
        req += &format!("{k}: {v}\r\n");
    }
    let mut wire = Vec::new();
    if chunked {
        req += "Transfer-Encoding: chunked\r\n\r\n";
        wire.extend_from_slice(req.as_bytes());
        for c in body.chunks(4 * KIB) {
            wire.extend_from_slice(format!("{:x}\r\n", c.len()).as_bytes());
            wire.extend_from_slice(c);
            wire.extend_from_slice(b"\r\n");
        }
        wire.extend_from_slice(b"0\r\n\r\n");
    } else {
        req += &format!("Content-Length: {}\r\n\r\n", body.len());
        wire.extend_from_slice(req.as_bytes());
        wire.extend_from_slice(body);
    }
    // The server answers (and closes) without waiting for the whole body in some cases.
    let _ = s.write_all(&wire).await;
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out).await;
    let text = String::from_utf8_lossy(&out).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
    Resp {
        status,
        body: body.to_owned(),
    }
}

async fn start(limit: MemoryLimit, inline_threshold: usize) -> Server {
    let server = Server::start(Config {
        memory_limit: limit,
        inline_threshold,
        ..Config::default()
    })
    .await
    .unwrap();
    assert_eq!(send(&server, "PUT", "/bkt", &[], b"", false).await.status, 200);
    server
}

async fn put(server: &Server, key: &str, len: usize) -> Resp {
    send(server, "PUT", &format!("/bkt/{key}"), &[], &vec![7u8; len], false).await
}

async fn delete(server: &Server, path: &str) {
    assert_eq!(send(server, "DELETE", path, &[], b"", false).await.status, 204);
}

fn used(server: &Server) -> u64 {
    server.memory().used
}

fn tag<'a>(xml: &'a str, name: &str) -> &'a str {
    let open = format!("<{name}>");
    let start = xml.find(&open).unwrap() + open.len();
    &xml[start..start + xml[start..].find('<').unwrap()]
}

#[tokio::test]
async fn heap_and_memfd_blobs_are_counted_and_released() {
    // 8 KiB threshold: 4 KiB lands on the heap, 16 KiB in a memfd.
    let server = start(MemoryLimit::Unlimited, 8 * KIB).await;
    assert_eq!(server.memory().limit, None);
    assert_eq!(used(&server), 0);
    assert_eq!(put(&server, "small", 4 * KIB).await.status, 200);
    assert_eq!(used(&server), 4 * KIB as u64);
    assert_eq!(put(&server, "big", 16 * KIB).await.status, 200);
    assert_eq!(used(&server), 20 * KIB as u64);
    assert_eq!(put(&server, "empty", 0).await.status, 200);
    assert_eq!(used(&server), 20 * KIB as u64);

    // Overwrite swaps the old buffer for the new one.
    assert_eq!(put(&server, "big", 10 * KIB).await.status, 200);
    assert_eq!(used(&server), 14 * KIB as u64);

    delete(&server, "/bkt/big").await;
    assert_eq!(used(&server), 4 * KIB as u64);
    delete(&server, "/bkt/small").await;
    assert_eq!(used(&server), 0);
}

#[tokio::test]
async fn copies_share_one_buffer_until_the_last_reference_goes() {
    let server = start(MemoryLimit::Unlimited, 8 * KIB).await;
    put(&server, "a", 16 * KIB).await;
    for dst in ["b", "c"] {
        let r = send(
            &server,
            "PUT",
            &format!("/bkt/{dst}"),
            &[("x-amz-copy-source", "/bkt/a")],
            b"",
            false,
        )
        .await;
        assert_eq!(r.status, 200);
    }
    assert_eq!(used(&server), 16 * KIB as u64, "copies are counted once");
    delete(&server, "/bkt/a").await;
    delete(&server, "/bkt/b").await;
    assert_eq!(used(&server), 16 * KIB as u64);
    delete(&server, "/bkt/c").await;
    assert_eq!(used(&server), 0);
}

#[tokio::test]
async fn versions_hold_memory_until_deleted() {
    let server = start(MemoryLimit::Unlimited, 8 * KIB).await;
    let on = b"<VersioningConfiguration><Status>Enabled</Status></VersioningConfiguration>";
    assert_eq!(send(&server, "PUT", "/bkt?versioning", &[], on, false).await.status, 200);
    put(&server, "k", 3 * KIB).await;
    put(&server, "k", 5 * KIB).await;
    assert_eq!(used(&server), 8 * KIB as u64);

    // A plain DELETE only adds a delete marker.
    delete(&server, "/bkt/k").await;
    assert_eq!(used(&server), 8 * KIB as u64);

    let listing = send(&server, "GET", "/bkt?versions", &[], b"", false).await.body;
    let mut ids = Vec::new();
    let mut rest = listing.as_str();
    while let Some(i) = rest.find("<Version>") {
        rest = &rest[i..];
        ids.push((tag(rest, "VersionId").to_owned(), tag(rest, "Size").parse::<u64>().unwrap()));
        rest = &rest["<Version>".len()..];
    }
    assert_eq!(ids.len(), 2);
    let mut left = used(&server);
    for (id, size) in ids {
        delete(&server, &format!("/bkt/k?versionId={id}")).await;
        left -= size;
        assert_eq!(used(&server), left);
    }
    assert_eq!(used(&server), 0);
}

#[tokio::test]
async fn multipart_parts_are_counted_until_abort_or_complete() {
    let server = start(MemoryLimit::Unlimited, 2 * KIB).await;
    let init = send(&server, "POST", "/bkt/mp?uploads", &[], b"", false).await;
    let id = tag(&init.body, "UploadId").to_owned();
    let part = |n: u32| format!("/bkt/mp?partNumber={n}&uploadId={id}");

    for n in 1..=2 {
        let r = send(&server, "PUT", &part(n), &[], &vec![1u8; 4 * KIB], false).await;
        assert_eq!(r.status, 200);
    }
    assert_eq!(used(&server), 8 * KIB as u64);
    // Re-uploading a part replaces it.
    assert_eq!(send(&server, "PUT", &part(2), &[], &vec![1u8; 3 * KIB], false).await.status, 200);
    assert_eq!(used(&server), 7 * KIB as u64);
    // A part copied from an object shares that object's buffer: no new memory.
    put(&server, "src", 6 * KIB).await;
    assert_eq!(used(&server), 13 * KIB as u64);
    let r = send(&server, "PUT", &part(3), &[("x-amz-copy-source", "/bkt/src")], b"", false).await;
    assert_eq!(r.status, 200);
    assert_eq!(used(&server), 13 * KIB as u64);

    delete(&server, &format!("/bkt/mp?uploadId={id}")).await;
    assert_eq!(used(&server), 6 * KIB as u64, "abort frees the parts but not the source");

    // Completing moves the parts into the object without copying them.
    let init = send(&server, "POST", "/bkt/mp2?uploads", &[], b"", false).await;
    let id2 = tag(&init.body, "UploadId").to_owned();
    let p = format!("/bkt/mp2?partNumber=1&uploadId={id2}");
    assert_eq!(send(&server, "PUT", &p, &[], &vec![1u8; 4 * KIB], false).await.status, 200);
    assert_eq!(used(&server), 10 * KIB as u64);
    let xml = b"<CompleteMultipartUpload><Part><PartNumber>1</PartNumber></Part></CompleteMultipartUpload>";
    let r = send(&server, "POST", &format!("/bkt/mp2?uploadId={id2}"), &[], xml, false).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(used(&server), 10 * KIB as u64);
    delete(&server, "/bkt/mp2").await;
    delete(&server, "/bkt/src").await;
    assert_eq!(used(&server), 0);
}

#[tokio::test]
async fn known_length_over_the_limit_is_refused_with_a_clear_error() {
    let server = start(MemoryLimit::Bytes(64 * KIB as u64), 8 * KIB).await;
    assert_eq!(server.memory().limit, Some(64 * KIB as u64));
    assert_eq!(put(&server, "a", 40 * KIB).await.status, 200);

    let r = put(&server, "b", 40 * KIB).await;
    assert_eq!(r.status, 507, "{}", r.body);
    assert_eq!(tag(&r.body, "Code"), "XFakeS3MemoryLimitExceeded");
    let msg = tag(&r.body, "Message");
    assert!(msg.contains("fakes3 memory limit"), "{msg}");
    assert!(msg.contains("--memory-limit") && msg.contains("FAKES3_MEMORY_LIMIT"), "{msg}");
    assert_eq!(used(&server), 40 * KIB as u64, "a refused write holds nothing");
    assert_eq!(send(&server, "HEAD", "/bkt/b", &[], b"", false).await.status, 404);

    // A body that doesn't fit isn't even asked for when the client waits for 100-continue.
    let r = send(&server, "PUT", "/bkt/c", &[("Expect", "100-continue")], &vec![0u8; 40 * KIB], false).await;
    assert_eq!(r.status, 507);

    // Freeing memory makes room again.
    delete(&server, "/bkt/a").await;
    assert_eq!(put(&server, "b", 40 * KIB).await.status, 200);
    assert_eq!(used(&server), 40 * KIB as u64);
}

#[tokio::test]
async fn unknown_length_stops_at_the_limit_and_releases_what_it_buffered() {
    let server = start(MemoryLimit::Bytes(64 * KIB as u64), 8 * KIB).await;
    assert_eq!(put(&server, "a", 20 * KIB).await.status, 200);
    let body = vec![3u8; 100 * KIB];
    let r = send(&server, "PUT", "/bkt/big", &[], &body, true).await;
    assert_eq!(r.status, 507, "{}", r.body);
    assert_eq!(tag(&r.body, "Code"), "XFakeS3MemoryLimitExceeded");
    assert_eq!(used(&server), 20 * KIB as u64);

    // Chunked bodies that fit work.
    let r = send(&server, "PUT", "/bkt/ok", &[], &vec![3u8; 30 * KIB], true).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(used(&server), 50 * KIB as u64);
}

#[tokio::test]
async fn multipart_parts_count_toward_the_limit() {
    let server = start(MemoryLimit::Bytes(32 * KIB as u64), 8 * KIB).await;
    let init = send(&server, "POST", "/bkt/mp?uploads", &[], b"", false).await;
    let id = tag(&init.body, "UploadId").to_owned();
    let part = |n: u32| format!("/bkt/mp?partNumber={n}&uploadId={id}");
    assert_eq!(send(&server, "PUT", &part(1), &[], &vec![0u8; 20 * KIB], false).await.status, 200);
    let r = send(&server, "PUT", &part(2), &[], &vec![0u8; 20 * KIB], false).await;
    assert_eq!(r.status, 507);
    assert_eq!(put(&server, "x", 20 * KIB).await.status, 507);
    delete(&server, &format!("/bkt/mp?uploadId={id}")).await;
    assert_eq!(put(&server, "x", 20 * KIB).await.status, 200);
}

/// Documented behavior: the new object is buffered while the old one is still readable, so
/// overwriting needs room for both.
#[tokio::test]
async fn overwrite_needs_room_for_old_and_new_at_once() {
    let server = start(MemoryLimit::Bytes(64 * KIB as u64), 8 * KIB).await;
    assert_eq!(put(&server, "k", 40 * KIB).await.status, 200);
    assert_eq!(put(&server, "k", 40 * KIB).await.status, 507);
    assert_eq!(put(&server, "k", 20 * KIB).await.status, 200);
    assert_eq!(used(&server), 20 * KIB as u64);
}

#[tokio::test]
async fn servers_account_independently() {
    let a = start(MemoryLimit::Unlimited, 8 * KIB).await;
    let b = start(MemoryLimit::Unlimited, 8 * KIB).await;
    put(&a, "k", 5 * KIB).await;
    assert_eq!((used(&a), used(&b)), (5 * KIB as u64, 0));
}
