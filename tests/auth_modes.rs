//! The two authentication modes. By default (`auth: false`) a request signed with any credentials,
//! in every signing flavour, is accepted unverified and acts as the bucket owner, an unsigned
//! request is anonymous and goes through ACLs, bucket policies and Block Public Access, and the
//! data-integrity checks that travel with a signature still apply. With `auth: true` signatures are
//! verified.

mod common;

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aws_sdk_s3::config::{BehaviorVersion, Credentials, Region};
use aws_sdk_s3::presigning::PresigningConfig;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{ChecksumAlgorithm, ChecksumMode, CompletedMultipartUpload, CompletedPart};
use common::{Resp, req, send};
use fakes3::{Config, RequestFilter, Server};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

async fn start() -> Server {
    Server::start(Config::default()).await.unwrap()
}

async fn start_strict() -> Server {
    Server::start(Config {
        auth: true,
        ..Config::default()
    })
    .await
    .unwrap()
}

fn random_credentials() -> (String, String) {
    (
        format!("AKIA{}", uuid::Uuid::new_v4().simple()).to_uppercase(),
        uuid::Uuid::new_v4().simple().to_string(),
    )
}

/// A client signing with credentials the server has never heard of.
fn client(endpoint: String) -> aws_sdk_s3::Client {
    let (access, secret) = random_credentials();
    aws_sdk_s3::Client::from_conf(
        aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .endpoint_url(endpoint)
            .region(Region::new("eu-west-3"))
            .credentials_provider(Credentials::new(access, secret, None, None, "test"))
            .force_path_style(true)
            .build(),
    )
}

/// A TCP forwarder that remembers what clients send, to check which signing mode a test used.
async fn recording_proxy(upstream: SocketAddr) -> (SocketAddr, Arc<Mutex<Vec<u8>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        loop {
            let (mut client, _) = listener.accept().await.unwrap();
            let mut server = TcpStream::connect(upstream).await.unwrap();
            let log = log.clone();
            tokio::spawn(async move {
                let (mut cr, mut cw) = client.split();
                let (mut sr, mut sw) = server.split();
                let up = async {
                    let mut buf = vec![0; 64 * 1024];
                    loop {
                        let n = cr.read(&mut buf).await.unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        // Request heads are all that matters; bodies would only bloat the log.
                        log.lock().unwrap().extend_from_slice(&buf[..n.min(1024)]);
                        if sw.write_all(&buf[..n]).await.is_err() {
                            break;
                        }
                    }
                    let _ = sw.shutdown().await;
                };
                let down = async {
                    let _ = tokio::io::copy(&mut sr, &mut cw).await;
                    let _ = cw.shutdown().await;
                };
                tokio::join!(up, down);
            });
        }
    });
    (addr, seen)
}

fn saw(seen: &Mutex<Vec<u8>>, needle: &str) -> bool {
    let text = String::from_utf8_lossy(&seen.lock().unwrap()).to_ascii_lowercase();
    text.contains(&needle.to_ascii_lowercase())
}

fn split_url(url: &str) -> (String, String) {
    let rest = url.split_once("://").unwrap().1;
    let target = &rest[rest.find('/').unwrap()..];
    match target.split_once('?') {
        Some((p, q)) => (p.to_owned(), q.to_owned()),
        None => (target.to_owned(), String::new()),
    }
}

/// A body the SDK cannot hold as one in-memory buffer, so it frames it as aws-chunked with a trailer.
fn streaming(data: &[u8]) -> ByteStream {
    ByteStream::from_body_1_x(http_body_util::Full::new(bytes::Bytes::copy_from_slice(data)))
}

fn data(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 31 % 251) as u8).collect()
}

#[tokio::test]
async fn sdk_requests_with_any_credentials() {
    let server = start().await;
    // Every client gets its own random key and secret, and they all see the same store.
    let (a, b) = (client(server.endpoint()), client(server.endpoint()));
    a.create_bucket().bucket("bkt").send().await.unwrap();
    a.put_object()
        .bucket("bkt")
        .key("dir/k")
        .body(ByteStream::from_static(b"hello"))
        .send()
        .await
        .unwrap();
    let got = b.get_object().bucket("bkt").key("dir/k").send().await.unwrap();
    assert_eq!(got.body.collect().await.unwrap().into_bytes().as_ref(), b"hello");
    b.copy_object()
        .bucket("bkt")
        .key("copy")
        .copy_source("bkt/dir/k")
        .send()
        .await
        .unwrap();
    let list = a.list_objects_v2().bucket("bkt").send().await.unwrap();
    assert_eq!(list.key_count(), Some(2));
    a.delete_object().bucket("bkt").key("copy").send().await.unwrap();
    assert_eq!(b.list_buckets().send().await.unwrap().buckets().len(), 1);
    server.shutdown().await;
}

#[tokio::test]
async fn sdk_streaming_uploads_with_checksum_trailers() {
    let server = start().await;
    let (proxy, seen) = recording_proxy(server.addr()).await;
    let s3 = client(format!("http://{proxy}"));
    s3.create_bucket().bucket("bkt").send().await.unwrap();

    // A body above the inline threshold, so it goes through the memfd path, with a trailing checksum.
    let big = data(12 << 20);
    for (key, alg) in [
        ("crc32", ChecksumAlgorithm::Crc32),
        ("sha256", ChecksumAlgorithm::Sha256),
        ("crc64", ChecksumAlgorithm::Crc64Nvme),
    ] {
        let put = s3
            .put_object()
            .bucket("bkt")
            .key(key)
            .checksum_algorithm(alg)
            .body(streaming(&big))
            .send()
            .await
            .unwrap();
        let head = s3
            .head_object()
            .bucket("bkt")
            .key(key)
            .checksum_mode(ChecksumMode::Enabled)
            .send()
            .await
            .unwrap();
        assert_eq!(head.content_length(), Some(big.len() as i64));
        assert_eq!(
            (put.checksum_crc32(), put.checksum_sha256(), put.checksum_crc64_nvme()),
            (head.checksum_crc32(), head.checksum_sha256(), head.checksum_crc64_nvme())
        );
        let got = s3.get_object().bucket("bkt").key(key).send().await.unwrap();
        assert!(
            got.body.collect().await.unwrap().into_bytes().as_ref() == big.as_slice(),
            "{key} body differs"
        );
    }
    // The SDK really sent aws-chunked bodies with trailers (the mode depends on the endpoint scheme).
    assert!(saw(&seen, "x-amz-content-sha256: STREAMING-"), "no streaming upload was sent");
    assert!(saw(&seen, "x-amz-trailer: x-amz-checksum-crc32"));

    // Multipart: every part carries its own trailing checksum.
    let first = data(5 << 20);
    let second = data(1000);
    let upload = s3
        .create_multipart_upload()
        .bucket("bkt")
        .key("multi")
        .checksum_algorithm(ChecksumAlgorithm::Crc32)
        .send()
        .await
        .unwrap();
    let id = upload.upload_id().unwrap();
    let mut parts = Vec::new();
    for (n, body) in [(1, &first), (2, &second)] {
        let part = s3
            .upload_part()
            .bucket("bkt")
            .key("multi")
            .upload_id(id)
            .part_number(n)
            .checksum_algorithm(ChecksumAlgorithm::Crc32)
            .body(streaming(body))
            .send()
            .await
            .unwrap();
        parts.push(
            CompletedPart::builder()
                .part_number(n)
                .e_tag(part.e_tag().unwrap())
                .checksum_crc32(part.checksum_crc32().unwrap())
                .build(),
        );
    }
    s3.complete_multipart_upload()
        .bucket("bkt")
        .key("multi")
        .upload_id(id)
        .multipart_upload(CompletedMultipartUpload::builder().set_parts(Some(parts)).build())
        .send()
        .await
        .unwrap();
    let got = s3.get_object().bucket("bkt").key("multi").send().await.unwrap();
    let got = got.body.collect().await.unwrap().into_bytes();
    assert_eq!(got.len(), first.len() + second.len());
    assert!(got[..first.len()] == first[..] && got[first.len()..] == second[..]);
    server.shutdown().await;
}

#[tokio::test]
async fn sdk_presigned_urls() {
    let server = start().await;
    let s3 = client(server.endpoint());
    s3.create_bucket().bucket("bkt").send().await.unwrap();
    let expiry = || PresigningConfig::expires_in(Duration::from_secs(300)).unwrap();

    let put = s3.put_object().bucket("bkt").key("up").presigned(expiry()).await.unwrap();
    let (path, query) = split_url(put.uri());
    let r = send(server.addr(), req("PUT", &path).query(&query).body(b"via presigned put").anon()).await;
    assert_eq!(r.status, 200, "{}", r.body);

    let get = s3.get_object().bucket("bkt").key("up").presigned(expiry()).await.unwrap();
    let (path, query) = split_url(get.uri());
    let r = send(server.addr(), req("GET", &path).query(&query).anon()).await;
    assert_eq!((r.status, r.body.as_str()), (200, "via presigned put"));

    // Parameters that are not part of the signature still apply.
    let get = s3
        .get_object()
        .bucket("bkt")
        .key("up")
        .response_content_type("text/plain")
        .presigned(expiry())
        .await
        .unwrap();
    let (path, query) = split_url(get.uri());
    let r = send(server.addr(), req("GET", &path).query(&query).anon()).await;
    assert_eq!((r.status, r.header("content-type")), (200, Some("text/plain")));
    server.shutdown().await;
}

#[tokio::test]
async fn presigned_urls_still_expire_on_the_server_clock() {
    let server = start().await;
    let s3 = client(server.endpoint());
    s3.create_bucket().bucket("bkt").send().await.unwrap();
    s3.put_object()
        .bucket("bkt")
        .key("k")
        .body(ByteStream::from_static(b"v"))
        .send()
        .await
        .unwrap();
    let url = s3
        .get_object()
        .bucket("bkt")
        .key("k")
        .presigned(PresigningConfig::expires_in(Duration::from_secs(60)).unwrap())
        .await
        .unwrap();
    let (path, query) = split_url(url.uri());
    server.clock().freeze(None).unwrap();
    assert_eq!(send(server.addr(), req("GET", &path).query(&query).anon()).await.status, 200);
    server.clock().advance(2 * 60 * 1_000_000_000).unwrap();
    assert_eq!(send(server.addr(), req("GET", &path).query(&query).anon()).await.status, 403);
    server.shutdown().await;
}

const FAKE_SIG: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn v4_authorization(access: &str) -> String {
    format!("AWS4-HMAC-SHA256 Credential={access}/20240101/us-east-1/s3/aws4_request, SignedHeaders=host;x-amz-date, Signature={FAKE_SIG}")
}

#[tokio::test]
async fn signatures_are_not_checked() {
    let server = start().await;
    let addr = server.addr();
    assert_eq!(send(addr, req("PUT", "/bkt").unverified()).await.status, 200);

    // SigV4 header auth with a signature that cannot be right, and a skewed date.
    let auth = v4_authorization("SOMEKEY");
    let v4 = |method, path| {
        req(method, path)
            .anon()
            .header("authorization", &auth)
            .header("x-amz-date", "20240101T000000Z")
            .header("x-amz-content-sha256", "UNSIGNED-PAYLOAD")
    };
    let r = send(addr, v4("PUT", "/bkt/v4").body(b"v4")).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(send(addr, v4("GET", "/bkt/v4")).await.body, "v4");

    // SigV2 header auth.
    let v2 = |method, path| {
        req(method, path)
            .anon()
            .header("authorization", "AWS ANYKEY:c2lnbmF0dXJl")
            .header("date", "Mon, 01 Jan 2024 00:00:00 GMT")
    };
    let r = send(addr, v2("PUT", "/bkt/v2").body(b"v2")).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(send(addr, v2("GET", "/bkt/v2")).await.body, "v2");

    // SigV2 presigned URLs.
    let q = "AWSAccessKeyId=ANYKEY&Expires=4102444800&Signature=c2lnbmF0dXJl%3D";
    let r = send(addr, req("PUT", "/bkt/v2url").query(q).body(b"v2 url").anon()).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = send(addr, req("GET", "/bkt/v2url").query(q).anon()).await;
    assert_eq!((r.status, r.body.as_str()), (200, "v2 url"));

    // SigV4 presigned URLs that were never signed by anyone.
    let q = format!(
        "X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential=ANYKEY%2F20240101%2Fus-east-1%2Fs3%2Faws4_request&X-Amz-Date=20240101T000000Z&X-Amz-Expires=3600&X-Amz-SignedHeaders=host&X-Amz-Signature={FAKE_SIG}"
    );
    let r = send(addr, req("GET", "/bkt/v4").query(&q).anon()).await;
    assert_eq!((r.status, r.body.as_str()), (200, "v4"));

    // Unsigned requests are anonymous: not accepted on a private bucket.
    let r = send(addr, req("PUT", "/bkt/anon").body(b"anon").anon()).await;
    assert_eq!((r.status, r.code()), (403, "AccessDenied"), "{}", r.body);
    let r = send(addr, req("GET", "/bkt/v4").anon()).await;
    assert_eq!((r.status, r.code()), (403, "AccessDenied"), "{}", r.body);
    server.shutdown().await;
}

fn b64_sha256(data: &[u8]) -> String {
    base64_simd::STANDARD.encode_to_string(Sha256::digest(data))
}

/// An aws-chunked body of `chunks` followed by the final chunk and `trailer` lines, with every
/// signature a made-up value.
fn aws_chunked(chunks: &[&[u8]], signed: bool, trailer: &[String]) -> Vec<u8> {
    let sig = if signed {
        format!(";chunk-signature={FAKE_SIG}")
    } else {
        String::new()
    };
    let mut out = Vec::new();
    for c in chunks {
        out.extend_from_slice(format!("{:x}{sig}\r\n", c.len()).as_bytes());
        out.extend_from_slice(c);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("0{sig}\r\n").as_bytes());
    for line in trailer {
        out.extend_from_slice(format!("{line}\r\n").as_bytes());
    }
    if signed && !trailer.is_empty() {
        out.extend_from_slice(format!("x-amz-trailer-signature:{FAKE_SIG}\r\n").as_bytes());
    }
    out.extend_from_slice(b"\r\n");
    out
}

async fn put_chunked(addr: SocketAddr, key: &str, mode: &str, body: &[u8], decoded: usize, trailer: Option<&str>) -> Resp {
    let path = format!("/bkt/{key}");
    let auth = v4_authorization("CHUNKKEY");
    let decoded = decoded.to_string();
    let mut r = req("PUT", &path)
        .anon()
        .header("authorization", &auth)
        .header("x-amz-date", "20240101T000000Z")
        .header("x-amz-content-sha256", mode)
        .header("x-amz-decoded-content-length", &decoded)
        .header("content-encoding", "aws-chunked")
        .body(body);
    if let Some(name) = trailer {
        r = r.header("x-amz-trailer", name).header("x-amz-sdk-checksum-algorithm", "SHA256");
    }
    send(addr, r).await
}

#[tokio::test]
async fn streaming_payload_modes_with_trailing_checksums() {
    let server = start().await;
    let addr = server.addr();
    send(addr, req("PUT", "/bkt").unverified()).await;
    let (hello, world) = (&b"hello "[..], &b"world"[..]);
    let want = b64_sha256(b"hello world");
    let good = [format!("x-amz-checksum-sha256:{want}")];
    let bad = [format!("x-amz-checksum-sha256:{}", b64_sha256(b"something else"))];

    let modes = [
        ("STREAMING-AWS4-HMAC-SHA256-PAYLOAD", true, false),
        ("STREAMING-AWS4-HMAC-SHA256-PAYLOAD-TRAILER", true, true),
        ("STREAMING-UNSIGNED-PAYLOAD-TRAILER", false, true),
    ];
    for (mode, signed, has_trailer) in modes {
        let key = mode.to_ascii_lowercase();
        let name = has_trailer.then_some("x-amz-checksum-sha256");
        let trailer = if has_trailer { &good[..] } else { &[] };
        let body = aws_chunked(&[hello, world], signed, trailer);
        let r = put_chunked(addr, &key, mode, &body, 11, name).await;
        assert_eq!(r.status, 200, "{mode}: {}", r.body);
        let got = send(
            addr,
            req("GET", &format!("/bkt/{key}"))
                .header("x-amz-checksum-mode", "ENABLED")
                .unverified(),
        )
        .await;
        assert_eq!(got.body, "hello world", "{mode}");
        if has_trailer {
            assert_eq!(got.header("x-amz-checksum-sha256"), Some(want.as_str()), "{mode}");
        }
        assert_eq!(
            got.header("content-encoding"),
            None,
            "{mode}: aws-chunked is framing, not object metadata"
        );

        if has_trailer {
            // The trailing checksum is an integrity check, not part of the signature: it must hold.
            let key = format!("{key}-corrupt");
            let body = aws_chunked(&[hello, world], signed, &bad);
            let r = put_chunked(addr, &key, mode, &body, 11, name).await;
            assert_eq!((r.status, r.code()), (400, "BadDigest"), "{mode}: {}", r.body);
            assert_eq!(
                send(addr, req("GET", &format!("/bkt/{key}")).unverified()).await.status,
                404,
                "{mode}"
            );
        }

        // Fewer bytes than x-amz-decoded-content-length announced.
        let key = format!("{key}-short");
        let body = aws_chunked(&[hello, world], signed, trailer);
        let r = put_chunked(addr, &key, mode, &body, 12, name).await;
        assert!(r.status >= 400, "{mode}: {}", r.body);
        assert_eq!(
            send(addr, req("GET", &format!("/bkt/{key}")).unverified()).await.status,
            404,
            "{mode}"
        );

        // Truncated in the middle of a chunk.
        let key = format!("{key}-cut");
        let body = aws_chunked(&[hello, world], signed, trailer);
        let r = put_chunked(addr, &key, mode, &body[..body.len() / 2], 11, name).await;
        assert!(r.status >= 400, "{mode}: {}", r.body);
        assert_eq!(
            send(addr, req("GET", &format!("/bkt/{key}")).unverified()).await.status,
            404,
            "{mode}"
        );
    }
    server.shutdown().await;
}

#[tokio::test]
async fn request_log_shows_the_access_key() {
    let server = start().await;
    let (access, secret) = ("AKIATESTKEY0000000001", "secret");
    let s3 = aws_sdk_s3::Client::from_conf(
        aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .endpoint_url(server.endpoint())
            .region(Region::new("us-east-1"))
            .credentials_provider(Credentials::new(access, secret, None, None, "test"))
            .force_path_style(true)
            .build(),
    );
    s3.create_bucket().bucket("bkt").send().await.unwrap();
    s3.put_object()
        .bucket("bkt")
        .key("k")
        .body(ByteStream::from_static(b"v"))
        .send()
        .await
        .unwrap();
    let url = s3
        .get_object()
        .bucket("bkt")
        .key("k")
        .presigned(PresigningConfig::expires_in(Duration::from_secs(60)).unwrap())
        .await
        .unwrap();
    let (path, query) = split_url(url.uri());
    send(server.addr(), req("GET", &path).query(&query).anon()).await;
    send(server.addr(), req("GET", "/bkt/k").anon()).await;

    let (records, total) = server.requests(&RequestFilter::default());
    assert_eq!(total, 4);
    let keys: Vec<_> = records.iter().map(|r| r.access_key.as_deref()).collect();
    assert_eq!(keys, [Some(access), Some(access), Some(access), None]);
    server.shutdown().await;
}

// ---- authorization in the default mode ----------------------------------------------------------

use aws_sdk_s3::types::{BucketCannedAcl, ObjectCannedAcl, PublicAccessBlockConfiguration};

const PUBLIC_READ_POLICY: &str =
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":"s3:GetObject","Resource":"arn:aws:s3:::bkt/*"}]}"#;

async fn put(s3: &aws_sdk_s3::Client, key: &str, body: &'static [u8]) {
    s3.put_object()
        .bucket("bkt")
        .key(key)
        .body(ByteStream::from_static(body))
        .send()
        .await
        .unwrap();
}

/// A server with a bucket `bkt` holding `k`, and an SDK client signing with random credentials.
async fn with_object() -> (Server, aws_sdk_s3::Client) {
    let server = start().await;
    let s3 = client(server.endpoint());
    s3.create_bucket().bucket("bkt").send().await.unwrap();
    put(&s3, "k", b"v").await;
    (server, s3)
}

fn anon_get(server: &Server, path: &str) -> impl std::future::Future<Output = Resp> {
    send(server.addr(), req("GET", path).anon())
}

fn code(err: &aws_sdk_s3::error::SdkError<impl aws_sdk_s3::error::ProvideErrorMetadata, impl std::fmt::Debug>) -> String {
    use aws_sdk_s3::error::ProvideErrorMetadata;
    err.code().unwrap_or("").to_owned()
}

#[tokio::test]
async fn signed_requests_with_any_credentials_are_the_owner() {
    let (server, s3) = with_object().await;
    // Owner-only operations work for credentials the server has never seen.
    s3.put_bucket_policy()
        .bucket("bkt")
        .policy(PUBLIC_READ_POLICY)
        .send()
        .await
        .unwrap();
    assert!(s3.get_bucket_policy().bucket("bkt").send().await.is_ok());
    s3.put_bucket_acl()
        .bucket("bkt")
        .acl(BucketCannedAcl::PublicRead)
        .send()
        .await
        .unwrap();
    s3.put_object_acl()
        .bucket("bkt")
        .key("k")
        .acl(ObjectCannedAcl::Private)
        .send()
        .await
        .unwrap();
    s3.put_public_access_block()
        .bucket("bkt")
        .public_access_block_configuration(PublicAccessBlockConfiguration::builder().block_public_policy(true).build())
        .send()
        .await
        .unwrap();
    s3.delete_bucket_policy().bucket("bkt").send().await.unwrap();
    // A different random identity is the same owner.
    let other = client(server.endpoint());
    assert_eq!(other.list_buckets().send().await.unwrap().buckets().len(), 1);
    server.shutdown().await;
}

#[tokio::test]
async fn unsigned_requests_are_anonymous() {
    let (server, s3) = with_object().await;
    for path in ["/", "/bkt", "/bkt/k"] {
        let r = anon_get(&server, path).await;
        assert_eq!((r.status, r.code()), (403, "AccessDenied"), "{path}: {}", r.body);
    }
    // Anonymous callers cannot create buckets, nor manage a bucket's policy, ACL or ownership.
    let r = send(server.addr(), req("PUT", "/anon-made").anon()).await;
    assert_eq!((r.status, r.code()), (403, "AccessDenied"));
    for (query, body) in [
        ("policy", PUBLIC_READ_POLICY),
        ("acl", ""),
        (
            "ownershipControls",
            "<OwnershipControls><Rule><ObjectOwnership>BucketOwnerPreferred</ObjectOwnership></Rule></OwnershipControls>",
        ),
    ] {
        let r = send(
            server.addr(),
            req("PUT", "/bkt")
                .query(query)
                .body(body.as_bytes())
                .header("x-amz-acl", "public-read-write")
                .anon(),
        )
        .await;
        assert_eq!((r.status, r.code()), (403, "AccessDenied"), "{query}: {}", r.body);
    }
    let r = anon_get(&server, "/missing/k").await;
    assert_eq!((r.status, r.code()), (404, "NoSuchBucket"));
    assert!(s3.get_bucket_policy().bucket("bkt").send().await.is_err(), "nothing was attached");
    server.shutdown().await;
}

#[tokio::test]
async fn public_read_policy_or_acl_opens_the_bucket_to_unsigned_requests() {
    let (server, s3) = with_object().await;
    assert_eq!(anon_get(&server, "/bkt/k").await.status, 403);

    s3.put_bucket_policy()
        .bucket("bkt")
        .policy(PUBLIC_READ_POLICY)
        .send()
        .await
        .unwrap();
    let r = anon_get(&server, "/bkt/k").await;
    assert_eq!((r.status, r.body.as_str()), (200, "v"));
    // The policy grants GetObject only.
    assert_eq!(anon_get(&server, "/bkt").await.status, 403);
    s3.delete_bucket_policy().bucket("bkt").send().await.unwrap();
    assert_eq!(anon_get(&server, "/bkt/k").await.status, 403);

    // The legacy ACL route: an object ACL, then a bucket ACL that allows listing.
    s3.put_object_acl()
        .bucket("bkt")
        .key("k")
        .acl(ObjectCannedAcl::PublicRead)
        .send()
        .await
        .unwrap();
    assert_eq!(anon_get(&server, "/bkt/k").await.status, 200);
    assert_eq!(anon_get(&server, "/bkt").await.status, 403);
    s3.put_bucket_acl()
        .bucket("bkt")
        .acl(BucketCannedAcl::PublicRead)
        .send()
        .await
        .unwrap();
    assert_eq!(anon_get(&server, "/bkt").await.status, 200);
    server.shutdown().await;
}

#[tokio::test]
async fn block_public_access_still_blocks() {
    let (server, s3) = with_object().await;
    let block = |config: PublicAccessBlockConfiguration| {
        s3.put_public_access_block()
            .bucket("bkt")
            .public_access_block_configuration(config)
            .send()
    };

    s3.put_object_acl()
        .bucket("bkt")
        .key("k")
        .acl(ObjectCannedAcl::PublicRead)
        .send()
        .await
        .unwrap();
    assert_eq!(anon_get(&server, "/bkt/k").await.status, 200);
    block(PublicAccessBlockConfiguration::builder().ignore_public_acls(true).build())
        .await
        .unwrap();
    assert_eq!(anon_get(&server, "/bkt/k").await.status, 403, "IgnorePublicAcls");
    // The owner is not affected.
    assert!(s3.get_object().bucket("bkt").key("k").send().await.is_ok());

    // Back to a private object, so that only the policy can open it.
    s3.put_object_acl()
        .bucket("bkt")
        .key("k")
        .acl(ObjectCannedAcl::Private)
        .send()
        .await
        .unwrap();
    block(PublicAccessBlockConfiguration::builder().ignore_public_acls(false).build())
        .await
        .unwrap();
    s3.put_bucket_policy()
        .bucket("bkt")
        .policy(PUBLIC_READ_POLICY)
        .send()
        .await
        .unwrap();
    assert_eq!(anon_get(&server, "/bkt/k").await.status, 200);
    block(PublicAccessBlockConfiguration::builder().restrict_public_buckets(true).build())
        .await
        .unwrap();
    assert_eq!(anon_get(&server, "/bkt/k").await.status, 403, "RestrictPublicBuckets");
    assert!(s3.get_object().bucket("bkt").key("k").send().await.is_ok());
    server.shutdown().await;
}

#[tokio::test]
async fn explicit_deny_applies_to_anonymous_requests() {
    let (server, s3) = with_object().await;
    put(&s3, "secret/k", b"s").await;
    let policy = r#"{"Version":"2012-10-17","Statement":[
        {"Effect":"Allow","Principal":"*","Action":"s3:GetObject","Resource":"arn:aws:s3:::bkt/*"},
        {"Effect":"Deny","Principal":"*","Action":"s3:GetObject","Resource":"arn:aws:s3:::bkt/secret/*"}]}"#;
    s3.put_bucket_policy().bucket("bkt").policy(policy).send().await.unwrap();
    assert_eq!(anon_get(&server, "/bkt/k").await.status, 200);
    assert_eq!(anon_get(&server, "/bkt/secret/k").await.status, 403);
    // A public-read ACL does not get around the Deny.
    s3.put_object_acl()
        .bucket("bkt")
        .key("secret/k")
        .acl(ObjectCannedAcl::PublicRead)
        .send()
        .await
        .unwrap();
    assert_eq!(anon_get(&server, "/bkt/secret/k").await.status, 403);
    // Signed requests are held to it as well, except for managing the policy itself.
    let err = s3.get_object().bucket("bkt").key("secret/k").send().await.unwrap_err();
    assert_eq!(code(&err), "AccessDenied");
    s3.delete_bucket_policy().bucket("bkt").send().await.unwrap();
    assert_eq!(anon_get(&server, "/bkt/secret/k").await.status, 200);
    server.shutdown().await;
}

#[tokio::test]
async fn presigned_urls_are_query_string_auth_for_policy_conditions() {
    let (server, s3) = with_object().await;
    let policy = r#"{"Version":"2012-10-17","Statement":[
        {"Effect":"Deny","Principal":"*","Action":"s3:GetObject","Resource":"arn:aws:s3:::bkt/*","Condition":{"StringEquals":{"s3:authType":"REST-QUERY-STRING"}}}]}"#;
    s3.put_bucket_policy().bucket("bkt").policy(policy).send().await.unwrap();
    let url = s3
        .get_object()
        .bucket("bkt")
        .key("k")
        .presigned(PresigningConfig::expires_in(Duration::from_secs(60)).unwrap())
        .await
        .unwrap();
    let (path, query) = split_url(url.uri());
    assert_eq!(send(server.addr(), req("GET", &path).query(&query).anon()).await.status, 403);
    assert!(s3.get_object().bucket("bkt").key("k").send().await.is_ok());
    server.shutdown().await;
}

#[tokio::test]
async fn every_authorization_entry_point_sees_unsigned_requests_as_anonymous() {
    let (server, s3) = with_object().await;
    s3.put_bucket_acl()
        .bucket("bkt")
        .acl(BucketCannedAcl::PublicReadWrite)
        .send()
        .await
        .unwrap();
    s3.create_bucket().bucket("priv").send().await.unwrap();
    put_to(&s3, "priv", "src", b"private").await;
    let addr = server.addr();

    // Anonymous writes are allowed by the bucket's public-write ACL.
    let r = send(addr, req("PUT", "/bkt/anon").body(b"anon").anon()).await;
    assert_eq!(r.status, 200, "{}", r.body);

    // CopyObject checks the source too: a private one cannot be read by an anonymous caller.
    let r = send(addr, req("PUT", "/bkt/stolen").header("x-amz-copy-source", "/priv/src").anon()).await;
    assert_eq!((r.status, r.code()), (403, "AccessDenied"), "{}", r.body);
    let r = send(addr, req("PUT", "/bkt/copied").header("x-amz-copy-source", "/bkt/k").anon()).await;
    assert_eq!(r.status, 403, "reading k needs a grant the ACL does not give: {}", r.body);

    // DeleteObjects is judged per key.
    let policy = r#"{"Version":"2012-10-17","Statement":[
        {"Effect":"Deny","Principal":"*","Action":"s3:DeleteObject","Resource":"arn:aws:s3:::bkt/k"}]}"#;
    s3.put_bucket_policy().bucket("bkt").policy(policy).send().await.unwrap();
    let body = b"<Delete><Object><Key>k</Key></Object><Object><Key>anon</Key></Object></Delete>";
    let sum = b64_sha256(body);
    let r = send(
        addr,
        req("POST", "/bkt")
            .query("delete")
            .header("x-amz-checksum-sha256", &sum)
            .body(body)
            .anon(),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.body.contains("<Key>anon</Key></Deleted>"), "{}", r.body);
    assert!(r.body.contains("<Code>AccessDenied</Code><Key>k</Key>"), "{}", r.body);
    // Signed requests are held to the Deny too (it is a Deny for everyone).
    let r = send(
        addr,
        req("POST", "/bkt")
            .query("delete")
            .header("x-amz-checksum-sha256", &sum)
            .body(body)
            .unverified(),
    )
    .await;
    assert!(r.body.contains("<Code>AccessDenied</Code><Key>k</Key>"), "{}", r.body);

    // PostObject forms: allowed into the public-write bucket, refused into the private one.
    let form = |bucket: &str| {
        let body = "--B\r\nContent-Disposition: form-data; name=\"key\"\r\n\r\nposted\r\n--B\r\nContent-Disposition: form-data; name=\"file\"; filename=\"f\"\r\n\r\ndata\r\n--B--\r\n".to_owned();
        (format!("/{bucket}"), body)
    };
    for (bucket, want) in [("bkt", 204), ("priv", 403)] {
        let (path, body) = form(bucket);
        let r = send(
            addr,
            req("POST", &path)
                .header("content-type", "multipart/form-data; boundary=B")
                .body(body.as_bytes())
                .anon(),
        )
        .await;
        assert_eq!(r.status, want, "{bucket}: {}", r.body);
    }
    server.shutdown().await;
}

async fn put_to(s3: &aws_sdk_s3::Client, bucket: &str, key: &str, body: &'static [u8]) {
    s3.put_object()
        .bucket(bucket)
        .key(key)
        .body(ByteStream::from_static(body))
        .send()
        .await
        .unwrap();
}

// ---- `auth: true` -------------------------------------------------------------------------------

fn strict_client(server: &Server, secret: &str) -> aws_sdk_s3::Client {
    let config = Config::default();
    aws_sdk_s3::Client::from_conf(
        aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .endpoint_url(server.endpoint())
            .region(Region::new("us-east-1"))
            .credentials_provider(Credentials::new(&config.access_key, secret, None, None, "test"))
            .force_path_style(true)
            .build(),
    )
}

#[tokio::test]
async fn strict_mode_rejects_bad_signatures() {
    let server = start_strict().await;
    let good = strict_client(&server, &Config::default().secret_key);
    good.create_bucket().bucket("bkt").send().await.unwrap();
    put(&good, "k", b"v").await;

    let err = strict_client(&server, "wrong").list_buckets().send().await.unwrap_err();
    assert_eq!(code(&err), "SignatureDoesNotMatch");
    // An access key nobody configured.
    let err = client(server.endpoint()).list_buckets().send().await.unwrap_err();
    assert_eq!(code(&err), "NotSignedUp");
    // Hand-written signatures, in header and query form, are checked as well.
    let r = send(
        server.addr(),
        req("GET", "/bkt/k")
            .anon()
            .header("authorization", &v4_authorization("fakes3"))
            .header("x-amz-date", "20240101T000000Z"),
    )
    .await;
    assert_eq!(r.status, 403, "{}", r.body);
    let q = format!(
        "X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential=fakes3%2F20240101%2Fus-east-1%2Fs3%2Faws4_request&X-Amz-Date=20240101T000000Z&X-Amz-Expires=3600&X-Amz-SignedHeaders=host&X-Amz-Signature={FAKE_SIG}"
    );
    assert_eq!(send(server.addr(), req("GET", "/bkt/k").query(&q).anon()).await.status, 403);
    // A made-up Authorization header is no longer enough to be the owner.
    assert_eq!(send(server.addr(), req("GET", "/bkt/k").unverified()).await.status, 403);
    server.shutdown().await;
}

#[tokio::test]
async fn strict_mode_authorizes_like_the_default_mode() {
    let server = start_strict().await;
    let s3 = strict_client(&server, &Config::default().secret_key);
    s3.create_bucket().bucket("bkt").send().await.unwrap();
    put(&s3, "k", b"v").await;
    let r = anon_get(&server, "/bkt/k").await;
    assert_eq!((r.status, r.code()), (403, "AccessDenied"));
    s3.put_bucket_policy()
        .bucket("bkt")
        .policy(PUBLIC_READ_POLICY)
        .send()
        .await
        .unwrap();
    assert_eq!(anon_get(&server, "/bkt/k").await.status, 200);
    // Verified presigned URLs work and keep their query-string auth type.
    let url = s3
        .get_object()
        .bucket("bkt")
        .key("k")
        .presigned(PresigningConfig::expires_in(Duration::from_secs(60)).unwrap())
        .await
        .unwrap();
    let (path, query) = split_url(url.uri());
    assert_eq!(send(server.addr(), req("GET", &path).query(&query).anon()).await.status, 200);
    server.shutdown().await;
}
