//! fakes3: an in-memory S3 server for tests.
//!
//! The binary is a thin CLI over this library; tests can also run the server in-process.
//! It listens on a random free port by default, so parallel tests never collide.
//!
//! In async code, start it on the current Tokio runtime:
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
//!
//! Synchronous code can use [`Server::start_blocking`], which runs the server on a background
//! runtime and stops it when the returned handle is dropped.
//!
//! Clients need path-style addressing. By default any credentials work: a signed request is
//! accepted without checking the signature and acts as the account its access key belongs to (the
//! default account, the owner of everything, for keys nobody configured), an unsigned one is
//! anonymous and subject to ACLs, bucket policies and Block Public Access. With `auth` on, requests
//! must be signed with the credentials from [`Config`] (`fakes3` / `fakes3secret` unless changed)
//! or those of [`Config::users`]. Any region works. See `examples/aws_sdk.rs` for a complete setup
//! with `aws-sdk-s3`, and `docs/multi-user.md` for several accounts.

mod access;
mod acl;
mod archive;
mod blocking;
mod bucket_config;
mod bucket_defaults;
mod bucket_ops;
mod clock;
mod control;
mod cors;
mod digest;
mod faults;
mod host;
mod lifecycle;
mod linger;
mod listxml;
mod memory;
mod names;
mod noauth;
mod objext;
mod objops;
mod policy;
mod presign;
mod reqid;
mod reqlog;
mod s3impl;
mod s3s_compat;
mod select;
mod sigv2;
mod snapshot;
mod spill;
mod store;
mod users;

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder as ConnBuilder;
use memory::MemoryTracker;
use s3s::service::S3ServiceBuilder;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

pub use blocking::BlockingServer;
#[doc(hidden)]
pub use blocking::worker_threads;
pub use bucket_defaults::BucketDefaults;
pub use clock::{Clock, OutOfRange, parse_duration};
pub use control::{Control, ControlError, DurationArg, ResetOptions, SnapshotInfo, parse_fault_duration};
pub use faults::{Delay, FaultAction, FaultInfo, FaultKind, FaultRule, InjectedFault};
#[doc(hidden)]
pub use listxml::counters as list_fast_path_counters;
pub use memory::{MemoryLimit, MemoryStats};
pub use reqlog::{DEFAULT_CAPACITY as DEFAULT_REQUEST_LOG_CAPACITY, RequestFilter, RequestRecord, StatusFilter};
pub use snapshot::Counts;
pub use users::{User, UserConfig, UserId, Users};

/// Server settings. `Default` listens on a random localhost port with the default credentials.
#[derive(Clone, Debug)]
pub struct Config {
    /// Address to listen on.
    pub host: String,
    /// 0 picks a free port.
    pub port: u16,
    /// Credentials clients must sign with when `auth` is on. They belong to the first account
    /// (`fakes3`, account id `000000000000`), which owns everything that other accounts do not.
    pub access_key: String,
    pub secret_key: String,
    /// More accounts, for cross-account ACL, policy and ownership behaviour: each is a separate
    /// owner with its own credentials. See `docs/multi-user.md`.
    pub users: Vec<UserConfig>,
    /// Verify request signatures (SigV4 and SigV2) against the credentials of the accounts. Off by
    /// default: a request signed with any access key and secret is accepted unverified and acts as
    /// the account its access key belongs to (the first one if none does), and an unsigned request
    /// is anonymous. Authorization (ACLs, bucket policies, Block Public Access) applies either way.
    pub auth: bool,
    /// On Linux, objects and parts at or above this size are stored in a memfd instead of on the heap.
    pub inline_threshold: usize,
    /// Base domains for virtual-host-style addressing: `<bucket>.<domain>` selects the bucket.
    /// Path-style requests keep working on every host. Empty disables virtual-host addressing.
    pub domains: Vec<String>,
    /// Record S3 requests for the control plane's request log.
    pub request_log: bool,
    /// Requests kept by the log; the oldest are dropped first.
    pub request_log_capacity: usize,
    /// Cap on the object data held in memory (including uploaded parts). A write that would pass it
    /// is refused with HTTP 507 `XFakeS3MemoryLimitExceeded`. The default, [`MemoryLimit::Auto`],
    /// is 75% of the container's or machine's memory.
    pub memory_limit: MemoryLimit,
    /// The settings new buckets start with. [`BucketDefaults::Legacy`] (the default) leaves ACLs
    /// enabled and Block Public Access off; [`BucketDefaults::Aws`] applies the defaults AWS has
    /// for new buckets today. Buckets that already exist are not affected.
    pub bucket_defaults: BucketDefaults,
    /// Write ListObjects, ListObjectsV2 and ListObjectVersions bodies straight from the index
    /// instead of through s3s' serializer. The bytes are identical; this only exists to switch the
    /// shortcut off, for differential testing or as a fallback.
    pub fast_list: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".into(),
            port: 0,
            access_key: "fakes3".into(),
            secret_key: "fakes3secret".into(),
            users: Vec::new(),
            auth: false,
            inline_threshold: 1 << 20,
            domains: vec!["localhost".into()],
            request_log: true,
            request_log_capacity: DEFAULT_REQUEST_LOG_CAPACITY,
            memory_limit: MemoryLimit::Auto,
            bucket_defaults: BucketDefaults::Legacy,
            fast_list: true,
        }
    }
}

/// A running server. [`Server::shutdown`] stops it and waits; dropping it also closes the
/// listener, but does not wait. Connections that are already open keep being served until their
/// clients hang up or the Tokio runtime shuts down.
pub struct Server {
    addr: SocketAddr,
    control: Arc<Control>,
    stop: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

impl Server {
    /// Binds the listener and starts serving on the current Tokio runtime.
    pub async fn start(config: Config) -> std::io::Result<Self> {
        let listener = TcpListener::bind((config.host.as_str(), config.port)).await?;
        let addr = listener.local_addr()?;
        let limit = config.memory_limit.resolve();
        match limit {
            Some(n) => tracing::info!("memory limit: {} ({:?})", memory::human(n), config.memory_limit),
            None => tracing::info!("memory limit: none"),
        }
        let users = Arc::new(
            Users::new(&config.access_key, &config.secret_key, &config.users)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?,
        );
        let control = Arc::new(Control::new(
            Arc::default(),
            Arc::default(),
            MemoryTracker::new(limit),
            reqlog::RequestLog::new(config.request_log, config.request_log_capacity),
            config.bucket_defaults,
            config.auth,
            users.clone(),
        ));
        let (stop, mut stopped) = oneshot::channel();
        let service = service(&config, control.clone());
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
                let abort = linger::Abort::default();
                let conn = http
                    .serve_connection(
                        TokioIo::new(linger::Lingering::new(socket, abort.clone())),
                        service.for_conn(peer.ip(), abort),
                    )
                    .into_owned();
                tokio::spawn(async move {
                    let _ = conn.await;
                });
            }
        });
        Ok(Self {
            addr,
            control,
            stop: Some(stop),
            task,
        })
    }

    /// The address actually bound; the real port when `Config::port` was 0.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// `http://host:port`, ready to use as an S3 endpoint URL.
    pub fn endpoint(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// The server's clock, for tests that need to move time.
    pub fn clock(&self) -> &Clock {
        &self.control.clock
    }

    /// Reset, snapshots and the request log: the same operations as the `/_fakes3/` HTTP API.
    pub fn control(&self) -> &Control {
        &self.control
    }

    /// Clears buckets, objects, uploads, the request log, the clock and the fault rules; keeps
    /// snapshots.
    pub fn reset(&self) {
        self.control.reset();
    }

    /// Adds a fault rule: matching S3 requests get an error, a delay, a truncated body or a reset
    /// connection. Rules are checked in the order they were added; the first that fires wins.
    ///
    /// ```no_run
    /// # fn demo(server: &fakes3::Server) -> Result<(), fakes3::ControlError> {
    /// // The first GetObject gets 503 SlowDown; the client's retry succeeds.
    /// server.add_fault(fakes3::FaultRule::slow_down().operation("GetObject").times(1))?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn add_fault(&self, rule: FaultRule) -> Result<FaultInfo, ControlError> {
        self.control.add_fault(rule)
    }

    /// The fault rules with their counters, in the order they are checked.
    pub fn faults(&self) -> Vec<FaultInfo> {
        self.control.faults()
    }

    /// Removes one fault rule.
    pub fn remove_fault(&self, id: u64) -> Result<(), ControlError> {
        self.control.remove_fault(id)
    }

    /// Removes all fault rules.
    pub fn clear_faults(&self) {
        self.control.clear_faults();
    }

    /// Copies the current buckets, objects and uploads under `name` (generated when `None`).
    pub fn snapshot(&self, name: Option<&str>) -> Result<SnapshotInfo, ControlError> {
        self.control.snapshot(name)
    }

    /// Rolls the store back to a snapshot.
    pub fn restore(&self, name: &str) -> Result<SnapshotInfo, ControlError> {
        self.control.restore(name)
    }

    /// Creates an empty bucket owned by the default account, as CreateBucket without options
    /// would. For clients that cannot create buckets themselves, such as OpenDAL.
    pub fn create_bucket(&self, name: &str) -> Result<(), ControlError> {
        self.control.create_bucket(name)
    }

    /// Turns versioning on for a bucket.
    pub fn enable_versioning(&self, bucket: &str) -> Result<(), ControlError> {
        self.control.enable_versioning(bucket)
    }

    /// Logged S3 requests matching `filter`, oldest first, and the match count before `limit`.
    pub fn requests(&self, filter: &RequestFilter) -> (Vec<RequestRecord>, usize) {
        self.control.requests(filter)
    }

    /// The accounts, the default one first, with the ids that were generated for those configured
    /// without them.
    pub fn users(&self) -> &Users {
        &self.control.users
    }

    /// Bytes of object data currently held, and the limit in force.
    pub fn memory(&self) -> MemoryStats {
        self.control.memory.stats()
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
    dyn Fn(http::Request<hyper::body::Incoming>, &Conn) -> futures::future::BoxFuture<'static, Result<s3s::HttpResponse, s3s::HttpError>>
        + Send
        + Sync,
>;

/// What the service knows about the connection a request came on.
#[derive(Clone)]
struct Conn {
    peer: IpAddr,
    /// Set by a reset fault: the connection then closes with a TCP reset.
    abort: linger::Abort,
}

struct Shared {
    s3: s3s::service::S3Service,
    control: Arc<Control>,
    domains: host::Domains,
    /// Whether signatures are verified; see [`noauth`] for what happens without.
    auth: bool,
}

fn service(config: &Config, control: Arc<Control>) -> ServiceFn {
    let fake = s3impl::FakeS3 {
        store: control.store.clone(),
        inline_threshold: config.inline_threshold,
        memory: control.memory.clone(),
        clock: control.clock.clone(),
        defaults: config.bucket_defaults,
        users: control.users.clone(),
        fast_list: config.fast_list,
    };
    let mut b = S3ServiceBuilder::new(fake);
    // Legacy clients (s3fs/aiobotocore presigning, old SDKs) still sign with SigV2.
    let mut s3_config = s3s::config::S3Config::default();
    s3_config.enable_sig_v2 = true;
    // A request signed with a `Date` header gets an `x-amz-date` that is not in its signed
    // headers (see [`s3s_compat::date_to_amz_date`]); the signature covers the time anyway.
    s3_config.unsigned_amz_header_allowlist.push("x-amz-date".to_owned());
    b.set_config(Arc::new(s3s::config::StaticConfigProvider::new(Arc::new(s3_config))));
    let domains = host::Domains::new(&config.domains);
    if !config.domains.is_empty() {
        b.set_host(domains.clone());
    }
    // s3s runs the access hook only when an auth provider is set. Without `auth` it never consults
    // it for the requests it sees (see [`noauth`]), but a signed POST form still reaches it.
    b.set_auth(control.users.auth());
    b.set_access(access::Access {
        store: control.store.clone(),
        clock: control.clock.clone(),
        users: control.users.clone(),
    });
    let shared = Arc::new(Shared {
        s3: b.build(),
        control,
        domains,
        auth: config.auth,
    });
    let f: Service = Arc::new(move |mut req: http::Request<hyper::body::Incoming>, conn: &Conn| {
        let sh = shared.clone();
        let peer = conn.peer;
        // With no fault rules this is one atomic load. Control-plane requests are never hit.
        let fault = if sh.control.faults.is_armed() && !req.uri().path().starts_with(control::PREFIX) {
            sh.control.faults.pick(&req, &sh.domains, &conn.abort)
        } else {
            None
        };
        Box::pin(async move {
            let control = &sh.control;
            if req.uri().path().starts_with(control::PREFIX) {
                return Ok(control.handle(req).await);
            }
            let id = reqid::RequestId::next();
            let present = names::scan(req.headers());
            let mut logged = control.log.start(&req, id, present, || control.clock.now());
            let mut truncate = None;
            let mut injected = None;
            if let Some(f) = fault {
                match inject(f, &req, logged.as_mut()).await {
                    Injection::Respond(r) => injected = Some(r),
                    Injection::Truncate(n) => truncate = Some(n),
                    Injection::Reset => {
                        if let Some(p) = logged {
                            control.log.finish_without_response(p);
                        }
                        return Err(s3s::HttpError::from_std_error("connection reset by a fault rule".into()));
                    }
                    Injection::Proceed => {}
                }
            }
            merge_duplicate_headers(req.headers_mut());
            let mut resp = if let Some(r) = injected {
                r
            } else if req.method() == http::Method::OPTIONS {
                // s3s does not route OPTIONS; preflights are answered from the bucket's CORS rules.
                cors::preflight(&control.store, &sh.domains, &req)
            } else {
                if control.store.access.record_peers.load(std::sync::atomic::Ordering::Relaxed) {
                    req.extensions_mut().insert(access::PeerAddr(peer));
                }
                // Only requests from a browser context carry Origin; everything else skips CORS.
                let cors_ctx = req
                    .headers()
                    .contains_key(http::header::ORIGIN)
                    .then(|| cors::RequestInfo::of(&req, &sh.domains));
                let head = req.method() == http::Method::HEAD;
                let mut req = req.map(s3s::Body::from);
                let signed_form = req.method() == http::Method::POST && s3s_compat::signed_form(&mut req).await;
                // s3s judges presigned-URL expiry by the host clock; honor a moved server clock first.
                let expired = control.clock.is_virtual() && req.uri().query().is_some_and(|q| presign::is_expired(q, control.clock.now()));
                let mut resp = if expired {
                    presign::expired_response()
                } else if let Some(refused) = digest::empty_content_md5(req.headers(), present) {
                    refused
                } else if sh.auth {
                    call_verified(&sh.s3, req).await?
                } else {
                    let signed = noauth::accept_any(&mut req, &control.users);
                    if let (Some(key), Some(p)) = (signed.and_then(|s| s.access_key.as_deref()), logged.as_mut()) {
                        p.set_access_key(key);
                    }
                    sh.s3.call(req).await?
                };
                // s3s always answers HEAD with 200; a ?partNumber= HEAD is a partial response.
                if head && resp.status() == http::StatusCode::OK && resp.headers().contains_key(http::header::CONTENT_RANGE) {
                    *resp.status_mut() = http::StatusCode::PARTIAL_CONTENT;
                }
                if signed_form && resp.status() == http::StatusCode::BAD_REQUEST {
                    s3s_compat::policy_violation(&mut resp);
                }
                if resp.status() == http::StatusCode::SEE_OTHER {
                    quote_redirect_etag(&mut resp);
                }
                if let Some(archive::StatusOverride(status)) = resp.extensions().get().copied() {
                    *resp.status_mut() = status;
                }
                // A list handler rendered its own body; s3s' is an empty document. Do this before
                // anything reads the response (CORS, the request log), so they see the real one.
                if let Some(listxml::PreRendered(body)) = resp.extensions_mut().remove() {
                    if resp.headers().contains_key(http::header::CONTENT_LENGTH) {
                        resp.headers_mut().insert(http::header::CONTENT_LENGTH, body.len().into());
                    }
                    *resp.body_mut() = s3s::Body::from(body);
                }
                if let Some(info) = cors_ctx {
                    info.decorate(&control.store, &mut resp);
                }
                resp
            };
            id.decorate(&mut resp);
            if let Some(p) = logged {
                control.log.finish(p, &resp);
            }
            if let Some(n) = truncate {
                let body = std::mem::take(resp.body_mut());
                *resp.body_mut() = faults::Truncated::wrap(body, n);
            }
            Ok(resp)
        })
    });
    ServiceFn(
        f,
        Conn {
            peer: IpAddr::from([127, 0, 0, 1]),
            abort: linger::Abort::default(),
        },
    )
}

/// What a fired fault leaves the request path to do.
enum Injection {
    /// Handle the request normally (the fault was only a delay).
    Proceed,
    /// Send this response instead of handling the request.
    Respond(s3s::HttpResponse),
    Truncate(u64),
    Reset,
}

/// Applies the delay of a fault and marks the request in the log. Kept out of line, so the
/// request path's future does not grow by a timer it almost never needs.
#[cold]
fn inject<'a, B>(
    f: faults::Fired,
    req: &'a http::Request<B>,
    logged: Option<&'a mut reqlog::Pending>,
) -> futures::future::BoxFuture<'a, Injection>
where
    B: Sync,
{
    Box::pin(async move {
        if let Some(p) = logged {
            p.set_fault(f.injected);
        }
        if let Some(d) = f.delay {
            tokio::time::sleep(d).await;
        }
        match f.action {
            None => Injection::Proceed,
            Some(FaultAction::Error { status, code, message }) => {
                let resource = req.uri().path();
                let head = req.method() == http::Method::HEAD;
                Injection::Respond(faults::error_response(status, &code, message.as_deref(), resource, head))
            }
            Some(FaultAction::Truncate { bytes }) => Injection::Truncate(bytes),
            Some(FaultAction::Reset) => {
                if let Some(a) = f.abort {
                    a.set();
                }
                Injection::Reset
            }
        }
    })
}

#[derive(Clone)]
struct ServiceFn(Service, Conn);

impl ServiceFn {
    fn for_conn(&self, peer: IpAddr, abort: linger::Abort) -> Self {
        Self(self.0.clone(), Conn { peer, abort })
    }
}

impl hyper::service::Service<http::Request<hyper::body::Incoming>> for ServiceFn {
    type Response = s3s::HttpResponse;
    type Error = s3s::HttpError;
    type Future = futures::future::BoxFuture<'static, Result<s3s::HttpResponse, s3s::HttpError>>;

    fn call(&self, req: http::Request<hyper::body::Incoming>) -> Self::Future {
        (self.0)(req, &self.1)
    }
}

/// A POST form's `success_action_redirect` answer (the only 303 fakes3 sends) carries
/// `?bucket=…&key=…&etag=…` appended to the client's URL. s3s appends the ETag bare; S3 appends it
/// with its quotes (`etag=%22…%22`), as the `ETag` header has them.
fn quote_redirect_etag(resp: &mut s3s::HttpResponse) {
    let Some(location) = resp.headers().get(http::header::LOCATION).and_then(|v| v.to_str().ok()) else {
        return;
    };
    let Some(at) = location.rfind("&etag=") else { return };
    let (head, etag) = location.split_at(at + "&etag=".len());
    if etag.starts_with("%22") {
        return;
    }
    if let Ok(v) = http::HeaderValue::from_str(&format!("{head}%22{etag}%22")) {
        resp.headers_mut().insert(http::header::LOCATION, v);
    }
}

/// Calls s3s with signature verification on, after repairing what it gets wrong (see
/// [`sigv2`] and [`s3s_compat`]). A request that is neither SigV2 nor unsized goes straight in.
async fn call_verified(s3: &s3s::service::S3Service, mut req: http::Request<s3s::Body>) -> Result<s3s::HttpResponse, s3s::HttpError> {
    if let Some(kind) = sigv2::kind(&req) {
        if kind == sigv2::Kind::Header
            && let Some(refused) = sigv2::bad_date(req.headers(), std::time::SystemTime::now())
        {
            return Ok(refused);
        }
        if sigv2::is_bucket_level(&req) {
            return sigv2::call_retrying(s3, req).await;
        }
    } else {
        s3s_compat::date_to_amz_date(req.headers_mut());
        if s3s_compat::is_unsized(req.headers()) {
            return match s3s_compat::with_length(req).await {
                Ok(req) => s3.call(req).await,
                Err(refused) => Ok(*refused),
            };
        }
    }
    s3.call(req).await
}

/// Folds repeated headers into one comma-separated value, which is how HTTP defines their
/// meaning (RFC 9110 §5.3). Joined with a bare `,` because that is how SigV4 canonicalizes
/// repeated headers, so signatures still verify. Some SDKs (aws-sdk-java-v2) send
/// `Content-Encoding: gzip` and `Content-Encoding: aws-chunked` separately, which s3s rejects.
fn merge_duplicate_headers(h: &mut http::HeaderMap) {
    // `iter` yields the values of one name together, so a name that follows itself has several.
    // Looking up every name to count its values (what the slow path does) hashes them all.
    let mut prev: Option<&http::HeaderName> = None;
    let mut repeated = false;
    for (name, _) in h.iter() {
        if prev == Some(name) && *name != http::header::HOST && *name != http::header::CONTENT_LENGTH {
            repeated = true;
            break;
        }
        prev = Some(name);
    }
    if !repeated {
        return;
    }
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
