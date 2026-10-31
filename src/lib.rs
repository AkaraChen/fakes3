//! fakes3: an in-memory S3 server for tests.
//!
//! The binary is a thin CLI over this library; tests can also run the server in-process:
//!
//! ```no_run
//! # async fn demo() -> std::io::Result<()> {
//! let server = fakes3::Server::start(fakes3::Config::default()).await?;
//! let endpoint = server.endpoint(); // "http://127.0.0.1:<random port>"
//! // ... point an S3 client at `endpoint` ...
//! server.shutdown().await;
//! # Ok(())
//! # }
//! ```

mod clock;
mod digest;
mod memory;
mod objext;
mod objops;
mod s3impl;
mod store;

use std::net::SocketAddr;
use std::sync::Arc;

use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder as ConnBuilder;
use memory::MemoryTracker;
use s3s::auth::SimpleAuth;
use s3s::service::S3ServiceBuilder;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

pub use clock::Clock;
pub use memory::{MemoryLimit, MemoryStats};

/// Server settings. `Default` listens on a random localhost port with the default credentials.
#[derive(Clone, Debug)]
pub struct Config {
    pub host: String,
    /// 0 picks a free port.
    pub port: u16,
    pub access_key: String,
    pub secret_key: String,
    /// Verify request signatures.
    pub auth: bool,
    /// Objects and parts at or above this size are stored in a memfd instead of on the heap.
    pub inline_threshold: usize,
    /// Cap on the object data held in memory (including uploaded parts). A write that would pass it
    /// is refused with HTTP 507 `XFakeS3MemoryLimitExceeded`. The default, [`MemoryLimit::Auto`],
    /// is 75% of the container's or machine's memory.
    pub memory_limit: MemoryLimit,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".into(),
            port: 0,
            access_key: "fakes3".into(),
            secret_key: "fakes3secret".into(),
            auth: true,
            inline_threshold: 1 << 20,
            memory_limit: MemoryLimit::Auto,
        }
    }
}

/// A running server. Dropping it without [`Server::shutdown`] leaves it running until the
/// Tokio runtime it was started on shuts down.
pub struct Server {
    addr: SocketAddr,
    clock: Arc<Clock>,
    memory: Arc<MemoryTracker>,
    stop: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

impl Server {
    /// Binds the listener and starts serving on the current Tokio runtime.
    pub async fn start(config: Config) -> std::io::Result<Self> {
        let listener = TcpListener::bind((config.host.as_str(), config.port)).await?;
        let addr = listener.local_addr()?;
        let clock = Arc::new(Clock::default());
        let (stop, mut stopped) = oneshot::channel();
        let limit = config.memory_limit.resolve();
        match limit {
            Some(n) => tracing::info!("memory limit: {} ({:?})", memory::human(n), config.memory_limit),
            None => tracing::info!("memory limit: none"),
        }
        let memory = MemoryTracker::new(limit);
        let service = service(&config, clock.clone(), memory.clone());
        let task = tokio::spawn(async move {
            let http = ConnBuilder::new(TokioExecutor::new());
            loop {
                let socket = tokio::select! {
                    r = listener.accept() => match r {
                        Ok((s, _)) => s,
                        Err(e) => {
                            tracing::warn!("accept: {e}");
                            continue;
                        }
                    },
                    _ = &mut stopped => break,
                };
                let _ = socket.set_nodelay(true);
                let conn = http.serve_connection(TokioIo::new(socket), service.clone()).into_owned();
                tokio::spawn(async move {
                    let _ = conn.await;
                });
            }
        });
        Ok(Self {
            addr,
            clock,
            memory,
            stop: Some(stop),
            task,
        })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// `http://host:port`, ready to use as an S3 endpoint URL.
    pub fn endpoint(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// The server's clock, for tests that need to move time.
    pub fn clock(&self) -> &Clock {
        &self.clock
    }

    /// Bytes of object data currently held, and the limit in force.
    pub fn memory(&self) -> MemoryStats {
        self.memory.stats()
    }

    /// Stops accepting connections. Requests already in flight finish on their own.
    pub async fn shutdown(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let _ = (&mut self.task).await;
    }

    /// Serves until the process exits.
    pub async fn wait(self) {
        let _ = self.task.await;
    }
}

type Service = Arc<
    dyn Fn(http::Request<hyper::body::Incoming>) -> futures::future::BoxFuture<'static, Result<s3s::HttpResponse, s3s::HttpError>>
        + Send
        + Sync,
>;

fn service(config: &Config, clock: Arc<Clock>, memory: Arc<MemoryTracker>) -> ServiceFn {
    let fake = s3impl::FakeS3 {
        store: store::Store::default(),
        inline_threshold: config.inline_threshold,
        memory,
        clock,
    };
    let mut b = S3ServiceBuilder::new(fake);
    // Legacy clients (s3fs/aiobotocore presigning, old SDKs) still sign with SigV2.
    let mut s3_config = s3s::config::S3Config::default();
    s3_config.enable_sig_v2 = true;
    b.set_config(Arc::new(s3s::config::StaticConfigProvider::new(Arc::new(s3_config))));
    if config.auth {
        b.set_auth(SimpleAuth::from_single(config.access_key.clone(), config.secret_key.clone()));
    }
    let s3 = b.build();
    let f: Service = Arc::new(move |mut req: http::Request<hyper::body::Incoming>| {
        let s3 = s3.clone();
        Box::pin(async move {
            merge_duplicate_headers(req.headers_mut());
            let head = req.method() == http::Method::HEAD;
            let mut resp = hyper::service::Service::call(&s3, req).await?;
            // s3s always answers HEAD with 200; a ?partNumber= HEAD is a partial response.
            if head && resp.status() == http::StatusCode::OK && resp.headers().contains_key(http::header::CONTENT_RANGE) {
                *resp.status_mut() = http::StatusCode::PARTIAL_CONTENT;
            }
            Ok(resp)
        })
    });
    ServiceFn(f)
}

#[derive(Clone)]
struct ServiceFn(Service);

impl hyper::service::Service<http::Request<hyper::body::Incoming>> for ServiceFn {
    type Response = s3s::HttpResponse;
    type Error = s3s::HttpError;
    type Future = futures::future::BoxFuture<'static, Result<s3s::HttpResponse, s3s::HttpError>>;

    fn call(&self, req: http::Request<hyper::body::Incoming>) -> Self::Future {
        (self.0)(req)
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
