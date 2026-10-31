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

mod access;
mod acl;
mod bucket_config;
mod bucket_ops;
mod clock;
mod cors;
mod digest;
mod host;
mod lifecycle;
mod memory;
mod policy;
mod s3impl;
mod store;

use std::net::{IpAddr, SocketAddr};
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
    /// Base domains for virtual-host-style addressing: `<bucket>.<domain>` selects the bucket.
    /// Path-style requests keep working on every host. Empty disables virtual-host addressing.
    pub domains: Vec<String>,
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
            domains: vec!["localhost".into()],
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
                        Ok(accepted) => accepted,
                        Err(e) => {
                            tracing::warn!("accept: {e}");
                            continue;
                        }
                    },
                    _ = &mut stopped => break,
                };
                let (socket, peer) = socket;
                let _ = socket.set_nodelay(true);
                let conn = http
                    .serve_connection(TokioIo::new(socket), service.for_peer(peer.ip()))
                    .into_owned();
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
    dyn Fn(http::Request<hyper::body::Incoming>, IpAddr) -> futures::future::BoxFuture<'static, Result<s3s::HttpResponse, s3s::HttpError>>
        + Send
        + Sync,
>;

struct Shared {
    s3: s3s::service::S3Service,
    store: Arc<store::Store>,
    domains: host::Domains,
}

fn service(config: &Config, clock: Arc<Clock>, memory: Arc<MemoryTracker>) -> ServiceFn {
    let store = Arc::new(store::Store::default());
    let fake = s3impl::FakeS3 {
        store: store.clone(),
        inline_threshold: config.inline_threshold,
        memory,
        clock: clock.clone(),
    };
    let mut b = S3ServiceBuilder::new(fake);
    // Legacy clients (s3fs/aiobotocore presigning, old SDKs) still sign with SigV2.
    let mut s3_config = s3s::config::S3Config::default();
    s3_config.enable_sig_v2 = true;
    b.set_config(Arc::new(s3s::config::StaticConfigProvider::new(Arc::new(s3_config))));
    let domains = host::Domains::new(&config.domains);
    if !config.domains.is_empty() {
        b.set_host(domains.clone());
    }
    if config.auth {
        b.set_auth(SimpleAuth::from_single(config.access_key.clone(), config.secret_key.clone()));
    }
    b.set_access(access::Access {
        store: store.clone(),
        clock,
        auth: config.auth,
    });
    let shared = Arc::new(Shared {
        s3: b.build(),
        store,
        domains,
    });
    let f: Service = Arc::new(move |mut req: http::Request<hyper::body::Incoming>, peer: IpAddr| {
        let sh = shared.clone();
        Box::pin(async move {
            merge_duplicate_headers(req.headers_mut());
            if req.method() == http::Method::OPTIONS {
                return Ok(cors::preflight(&sh.store, &sh.domains, &req));
            }
            if sh.store.access.record_peers.load(std::sync::atomic::Ordering::Relaxed) {
                req.extensions_mut().insert(access::PeerAddr(peer));
            }
            // Only requests from a browser context carry Origin; everything else skips CORS.
            let cors_ctx = req
                .headers()
                .contains_key(http::header::ORIGIN)
                .then(|| cors::RequestInfo::of(&req, &sh.domains));
            let head = req.method() == http::Method::HEAD;
            let mut resp = hyper::service::Service::call(&sh.s3, req).await?;
            // s3s always answers HEAD with 200; a ?partNumber= HEAD is a partial response.
            if head && resp.status() == http::StatusCode::OK && resp.headers().contains_key(http::header::CONTENT_RANGE) {
                *resp.status_mut() = http::StatusCode::PARTIAL_CONTENT;
            }
            if let Some(info) = cors_ctx {
                info.decorate(&sh.store, &mut resp);
            }
            Ok(resp)
        })
    });
    ServiceFn(f, IpAddr::from([127, 0, 0, 1]))
}

#[derive(Clone)]
struct ServiceFn(Service, IpAddr);

impl ServiceFn {
    fn for_peer(&self, peer: IpAddr) -> Self {
        Self(self.0.clone(), peer)
    }
}

impl hyper::service::Service<http::Request<hyper::body::Incoming>> for ServiceFn {
    type Response = s3s::HttpResponse;
    type Error = s3s::HttpError;
    type Future = futures::future::BoxFuture<'static, Result<s3s::HttpResponse, s3s::HttpError>>;

    fn call(&self, req: http::Request<hyper::body::Incoming>) -> Self::Future {
        (self.0)(req, self.1)
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
