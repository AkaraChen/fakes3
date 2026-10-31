mod digest;
mod s3impl;
mod store;

use std::io::Write;

use clap::Parser;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder as ConnBuilder;
use s3s::auth::SimpleAuth;
use s3s::service::S3ServiceBuilder;
use tokio::net::TcpListener;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[derive(Parser)]
#[command(version, about = "An in-memory S3 server for tests")]
struct Opt {
    #[arg(long, default_value = "127.0.0.1", env = "FAKES3_HOST")]
    host: String,

    /// Port to listen on; 0 picks a free port. The bound address is printed to stdout.
    #[arg(long, default_value_t = 9000, env = "FAKES3_PORT")]
    port: u16,

    #[arg(long, default_value = "fakes3", env = "FAKES3_ACCESS_KEY")]
    access_key: String,

    #[arg(long, default_value = "fakes3secret", env = "FAKES3_SECRET_KEY")]
    secret_key: String,

    /// Accept requests without verifying signatures.
    #[arg(long)]
    no_auth: bool,

    /// Objects and parts at or above this size are stored in a memfd instead of on the heap.
    #[arg(long, default_value_t = 1 << 20)]
    inline_threshold: usize,
}

fn main() -> std::io::Result<()> {
    let opt = Opt::parse();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(run(opt))
}

async fn run(opt: Opt) -> std::io::Result<()> {
    let fake = s3impl::FakeS3 {
        store: store::Store::default(),
        inline_threshold: opt.inline_threshold,
    };
    let mut b = S3ServiceBuilder::new(fake);
    // Legacy clients (s3fs/aiobotocore presigning, old SDKs) still sign with SigV2.
    let mut config = s3s::config::S3Config::default();
    config.enable_sig_v2 = true;
    b.set_config(std::sync::Arc::new(s3s::config::StaticConfigProvider::new(std::sync::Arc::new(
        config,
    ))));
    if !opt.no_auth {
        b.set_auth(SimpleAuth::from_single(opt.access_key, opt.secret_key));
    }
    let s3 = b.build();
    let service = hyper::service::service_fn(move |mut req: http::Request<hyper::body::Incoming>| {
        let s3 = s3.clone();
        async move {
            merge_duplicate_headers(req.headers_mut());
            let head = req.method() == http::Method::HEAD;
            let mut resp = hyper::service::Service::call(&s3, req).await?;
            // s3s always answers HEAD with 200; a ?partNumber= HEAD is a partial response.
            if head && resp.status() == http::StatusCode::OK && resp.headers().contains_key(http::header::CONTENT_RANGE) {
                *resp.status_mut() = http::StatusCode::PARTIAL_CONTENT;
            }
            Ok::<_, s3s::HttpError>(resp)
        }
    });

    let listener = TcpListener::bind((opt.host.as_str(), opt.port)).await?;
    let addr = listener.local_addr()?;
    println!("http://{addr}");
    std::io::stdout().flush()?;

    let http = ConnBuilder::new(TokioExecutor::new());
    loop {
        let (socket, _) = match listener.accept().await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("accept: {e}");
                continue;
            }
        };
        let _ = socket.set_nodelay(true);
        let conn = http.serve_connection(TokioIo::new(socket), service.clone()).into_owned();
        tokio::spawn(async move {
            let _ = conn.await;
        });
    }
}

/// Folds repeated headers into one comma-separated value, which is how HTTP defines their
/// meaning (RFC 9110 §5.3). Joined with a bare `,` because that is how SigV4 canonicalizes
/// repeated headers, so signatures still verify. Some SDKs (aws-sdk-java-v2) send
/// `Content-Encoding: gzip` and `Content-Encoding: aws-chunked` separately, which s3s rejects.
fn merge_duplicate_headers(h: &mut http::HeaderMap) {
    let dups: Vec<http::HeaderName> = h
        .keys()
        .filter(|k| *k != http::header::HOST && *k != http::header::CONTENT_LENGTH && h.get_all(*k).iter().nth(1).is_some())
        .cloned()
        .collect();
    for k in dups {
        let joined = h.get_all(&k).iter().map(|v| v.as_bytes()).collect::<Vec<_>>().join(&b","[..]);
        if let Ok(v) = http::HeaderValue::from_bytes(&joined) {
            h.insert(k, v);
        }
    }
}
