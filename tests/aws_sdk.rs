//! The in-process library API, exercised through the real `aws-sdk-s3` client.

use aws_sdk_s3::config::{BehaviorVersion, Credentials, Region};
use aws_sdk_s3::primitives::ByteStream;
use fakes3::{Config, Server};

fn client(endpoint: String, config: &Config) -> aws_sdk_s3::Client {
    aws_sdk_s3::Client::from_conf(
        aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .endpoint_url(endpoint)
            .region(Region::new("us-east-1"))
            .credentials_provider(Credentials::new(&config.access_key, &config.secret_key, None, None, "test"))
            .force_path_style(true)
            .build(),
    )
}

async fn roundtrip(s3: &aws_sdk_s3::Client) {
    s3.create_bucket().bucket("bucket").send().await.unwrap();
    s3.put_object()
        .bucket("bucket")
        .key("dir/k")
        .body(ByteStream::from_static(b"hello"))
        .send()
        .await
        .unwrap();
    let got = s3.get_object().bucket("bucket").key("dir/k").send().await.unwrap();
    assert_eq!(got.body.collect().await.unwrap().into_bytes().as_ref(), b"hello");
    let list = s3.list_objects_v2().bucket("bucket").prefix("dir/").send().await.unwrap();
    assert_eq!(list.key_count(), Some(1));
}

#[tokio::test]
async fn async_server_roundtrip() {
    let config = Config::default();
    let server = Server::start(config.clone()).await.unwrap();
    assert!(server.endpoint().starts_with("http://127.0.0.1:"));
    assert_ne!(server.addr().port(), 0);
    roundtrip(&client(server.endpoint(), &config)).await;
    server.shutdown().await;
}

#[tokio::test]
async fn servers_are_independent() {
    let config = Config::default();
    let a = Server::start(config.clone()).await.unwrap();
    let b = Server::start(config.clone()).await.unwrap();
    assert_ne!(a.addr(), b.addr());
    let (sa, sb) = (client(a.endpoint(), &config), client(b.endpoint(), &config));
    sa.create_bucket().bucket("only-in-a").send().await.unwrap();
    assert!(sb.list_buckets().send().await.unwrap().buckets().is_empty());
}

#[tokio::test]
async fn wrong_credentials_are_rejected() {
    let config = Config::default();
    let server = Server::start(config.clone()).await.unwrap();
    let wrong = Config {
        secret_key: "nope".into(),
        ..config
    };
    let err = client(server.endpoint(), &wrong).list_buckets().send().await.unwrap_err();
    assert_eq!(aws_sdk_s3::error::ProvideErrorMetadata::code(&err), Some("SignatureDoesNotMatch"));
}

#[tokio::test]
async fn no_auth_accepts_unsigned_requests() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let server = Server::start(Config {
        auth: false,
        ..Config::default()
    })
    .await
    .unwrap();
    let mut conn = tokio::net::TcpStream::connect(server.addr()).await.unwrap();
    conn.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut response = String::new();
    conn.read_to_string(&mut response).await.unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.contains("<ListAllMyBucketsResult"), "{response}");
}

#[tokio::test]
async fn fixed_port_and_rebind_after_shutdown() {
    let first = Server::start(Config::default()).await.unwrap();
    let port = first.addr().port();
    assert!(Server::start(Config { port, ..Config::default() }).await.is_err(), "port is taken");
    first.shutdown().await;
    let again = Server::start(Config { port, ..Config::default() }).await.unwrap();
    assert_eq!(again.addr().port(), port);
}

#[test]
fn blocking_server_from_sync_code() {
    let config = Config::default();
    let server = Server::start_blocking(config.clone()).unwrap();
    let endpoint = server.endpoint();
    assert_eq!(endpoint, format!("http://{}", server.addr()));

    // The client needs a runtime of its own; the server does not share it.
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(roundtrip(&client(endpoint, &config)));
}

#[test]
fn blocking_server_stops_on_drop() {
    let server = Server::start_blocking(Config::default()).unwrap();
    let addr = server.addr();
    assert!(std::net::TcpStream::connect(addr).is_ok());
    drop(server);
    // The listener is closed synchronously by the drop.
    assert!(std::net::TcpStream::connect(addr).is_err());
}

#[test]
fn blocking_shutdown_frees_the_port() {
    let server = Server::start_blocking(Config::default()).unwrap();
    let port = server.addr().port();
    server.shutdown();
    Server::start_blocking(Config { port, ..Config::default() }).unwrap().shutdown();
}

#[tokio::test]
async fn blocking_server_can_be_dropped_inside_async_code() {
    // Needs its own thread: starting a blocking server must not happen on a runtime thread.
    let server = std::thread::spawn(|| Server::start_blocking(Config::default()).unwrap())
        .join()
        .unwrap();
    let addr = server.addr();
    drop(server);
    assert!(tokio::net::TcpStream::connect(addr).await.is_err());
}
