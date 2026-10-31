//! Runs a server in-process and talks to it with `aws-sdk-s3`.
//!
//!     cargo run --example aws_sdk

use aws_sdk_s3::config::{BehaviorVersion, Credentials, Region};
use aws_sdk_s3::primitives::ByteStream;
use fakes3::{Config, Server};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Port 0 (the default) picks a free port, so concurrent tests never collide.
    let config = Config::default();
    let credentials = Credentials::new(&config.access_key, &config.secret_key, None, None, "example");
    let server = Server::start(config).await?;

    let s3 = aws_sdk_s3::Client::from_conf(
        aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .endpoint_url(server.endpoint())
            .region(Region::new("us-east-1"))
            .credentials_provider(credentials)
            .force_path_style(true)
            .build(),
    );

    s3.create_bucket().bucket("demo").send().await?;
    s3.put_object()
        .bucket("demo")
        .key("hello.txt")
        .body(ByteStream::from_static(b"hi"))
        .send()
        .await?;
    let body = s3.get_object().bucket("demo").key("hello.txt").send().await?.body.collect().await?;
    println!("{} -> {:?}", server.endpoint(), String::from_utf8_lossy(&body.into_bytes()));

    server.shutdown().await;
    Ok(())
}
