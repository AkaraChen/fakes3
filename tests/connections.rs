//! Connection handling that clients rely on, whatever the platform's TCP stack does.

mod common;

use std::time::Duration;

use fakes3::{Config, Server};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// An error answered before the request body is read reaches a client that sends the body only
/// afterwards. Closing the connection under the late body would reset it, and on macOS and Windows
/// the reset discards the response the client has not read yet (Linux delivers it first, so there
/// this passes either way).
#[tokio::test]
async fn an_early_error_survives_a_late_body() {
    let server = Server::start(Config::default()).await.unwrap();
    let addr = server.addr();
    let body = vec![b'x'; 256 << 10];
    let mut s = TcpStream::connect(addr).await.unwrap();
    let head = format!(
        "PUT /no-such-bucket/k HTTP/1.1\r\nhost: {addr}\r\nconnection: close\r\ncontent-length: {}\r\nauthorization: {}\r\n\r\n",
        body.len(),
        common::UNVERIFIED_AUTH
    );
    s.write_all(head.as_bytes()).await.unwrap();
    // NoSuchBucket is answered from the headers alone; send the body once that has happened.
    tokio::time::sleep(Duration::from_millis(200)).await;
    s.write_all(&body).await.unwrap();
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).await.unwrap();
    let text = String::from_utf8_lossy(&raw);
    assert!(text.starts_with("HTTP/1.1 404"), "{text}");
    assert!(text.contains("<Code>NoSuchBucket</Code>"), "{text}");
    server.shutdown().await;
}
