//! The test control plane: reset, snapshots, clock and request log, as methods on [`Control`]
//! (re-exported through `Server`) and as an HTTP API under `/_fakes3/` (see docs/control-plane.md).
//!
//! Bucket names cannot start with `_`, so a path-style S3 request never collides with the prefix.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use bytes::Bytes;
use http_body_util::{BodyExt, Limited};
use parking_lot::Mutex;
use s3s::dto::{Timestamp, TimestampFormat};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::bucket_defaults::BucketDefaults;
use crate::clock::{self, Clock};
use crate::memory::MemoryTracker;
use crate::reqlog::{RequestFilter, RequestLog, RequestRecord, StatusFilter};
use crate::snapshot::{Counts, StoreSnapshot};
use crate::store::Store;

pub const PREFIX: &str = "/_fakes3";

/// Request bodies larger than this are refused.
const MAX_BODY: usize = 1 << 20;

/// Shared state behind one server's control plane.
pub struct Control {
    pub(crate) store: Arc<Store>,
    pub(crate) clock: Arc<Clock>,
    pub(crate) memory: Arc<MemoryTracker>,
    pub(crate) log: RequestLog,
    pub(crate) bucket_defaults: BucketDefaults,
    /// Whether request signatures are verified (`Config::auth`).
    auth: bool,
    snapshots: Mutex<BTreeMap<String, SnapshotEntry>>,
    next_snapshot: AtomicU64,
}

struct SnapshotEntry {
    snap: Arc<StoreSnapshot>,
    created: SystemTime,
}

/// A stored snapshot, as listed.
#[derive(Clone, Debug)]
pub struct SnapshotInfo {
    pub name: String,
    /// Server clock time of the snapshot.
    pub created: SystemTime,
    pub counts: Counts,
}

/// What [`Control::reset_with`] clears. Buckets, objects and uploads are always cleared.
#[derive(Clone, Copy, Debug)]
pub struct ResetOptions {
    /// Put the clock back on the host clock.
    pub clock: bool,
    /// Empty the request log.
    pub requests: bool,
    /// Delete stored snapshots.
    pub snapshots: bool,
}

impl Default for ResetOptions {
    fn default() -> Self {
        Self {
            clock: true,
            requests: true,
            snapshots: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlError {
    BadRequest(String),
    NotFound(String),
    Conflict(String),
}

impl fmt::Display for ControlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadRequest(m) | Self::NotFound(m) | Self::Conflict(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for ControlError {}

impl From<clock::OutOfRange> for ControlError {
    fn from(e: clock::OutOfRange) -> Self {
        Self::BadRequest(e.to_string())
    }
}

impl Control {
    pub(crate) fn new(
        store: Arc<Store>,
        clock: Arc<Clock>,
        memory: Arc<MemoryTracker>,
        log: RequestLog,
        bucket_defaults: BucketDefaults,
        auth: bool,
    ) -> Self {
        Self {
            store,
            clock,
            memory,
            log,
            bucket_defaults,
            auth,
            snapshots: Mutex::default(),
            next_snapshot: AtomicU64::new(1),
        }
    }

    /// Clears buckets, objects, uploads, the request log and the clock; keeps snapshots.
    pub fn reset(&self) {
        self.reset_with(ResetOptions::default());
    }

    pub fn reset_with(&self, opts: ResetOptions) {
        self.store.clear();
        if opts.clock {
            self.clock.reset();
        }
        if opts.requests {
            self.log.clear();
        }
        if opts.snapshots {
            self.snapshots.lock().clear();
        }
    }

    /// Copies the current buckets, objects and uploads. `name` defaults to `snap-<n>`.
    pub fn snapshot(&self, name: Option<&str>) -> Result<SnapshotInfo, ControlError> {
        let name = match name {
            Some(n) => valid_name(n)?.to_owned(),
            None => format!("snap-{}", self.next_snapshot.fetch_add(1, Ordering::Relaxed)),
        };
        if self.snapshots.lock().contains_key(&name) {
            return Err(ControlError::Conflict(format!("snapshot {name:?} already exists")));
        }
        // Copy outside the registry lock: it can take a while for a big store.
        let snap = Arc::new(self.store.snapshot());
        let entry = SnapshotEntry {
            snap,
            created: self.clock.now(),
        };
        let info = entry.info(&name);
        match self.snapshots.lock().entry(name.clone()) {
            std::collections::btree_map::Entry::Occupied(_) => Err(ControlError::Conflict(format!("snapshot {name:?} already exists"))),
            std::collections::btree_map::Entry::Vacant(v) => {
                v.insert(entry);
                Ok(info)
            }
        }
    }

    /// Makes the store match snapshot `name` again. The snapshot, other snapshots, the clock and
    /// the request log are left alone.
    pub fn restore(&self, name: &str) -> Result<SnapshotInfo, ControlError> {
        let (snap, info) = {
            let snaps = self.snapshots.lock();
            let e = snaps.get(name).ok_or_else(|| not_found(name))?;
            (e.snap.clone(), e.info(name))
        };
        self.store.restore(&snap);
        Ok(info)
    }

    pub fn snapshots(&self) -> Vec<SnapshotInfo> {
        self.snapshots.lock().iter().map(|(n, e)| e.info(n)).collect()
    }

    pub fn delete_snapshot(&self, name: &str) -> Result<(), ControlError> {
        self.snapshots.lock().remove(name).map(drop).ok_or_else(|| not_found(name))
    }

    /// Logged requests matching `filter`, oldest first, and the match count before `limit`.
    pub fn requests(&self, filter: &RequestFilter) -> (Vec<RequestRecord>, usize) {
        self.log.list(filter)
    }

    pub fn clear_requests(&self) {
        self.log.clear();
    }

    pub fn set_request_log_enabled(&self, on: bool) {
        self.log.set_enabled(on);
    }

    pub fn set_request_log_capacity(&self, capacity: usize) {
        self.log.set_capacity(capacity);
    }

    /// Bytes of object data held and the memory limit.
    pub fn memory(&self) -> crate::memory::MemoryStats {
        self.memory.stats()
    }

    /// Bucket, object and upload counts.
    pub fn counts(&self) -> Counts {
        self.store.counts()
    }
}

impl SnapshotEntry {
    fn info(&self, name: &str) -> SnapshotInfo {
        SnapshotInfo {
            name: name.to_owned(),
            created: self.created,
            counts: self.snap.counts(),
        }
    }
}

fn not_found(name: &str) -> ControlError {
    ControlError::NotFound(format!("no snapshot named {name:?}"))
}

fn valid_name(n: &str) -> Result<&str, ControlError> {
    let ok = !n.is_empty() && n.len() <= 64 && n.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if ok {
        Ok(n)
    } else {
        Err(ControlError::BadRequest(
            "snapshot names are 1 to 64 characters from A-Z a-z 0-9 . _ -".into(),
        ))
    }
}

pub(crate) fn fmt_time(t: SystemTime) -> String {
    let mut out = Vec::new();
    // Formatting a representable time cannot fail; fall back to the epoch rather than panic.
    if Timestamp::from(t).format(TimestampFormat::DateTime, &mut out).is_err() {
        return "1970-01-01T00:00:00.000Z".into();
    }
    String::from_utf8(out).unwrap_or_default()
}

/// Parses an RFC 3339 time.
pub(crate) fn parse_rfc3339(s: &str) -> Option<SystemTime> {
    let t: time::OffsetDateTime = Timestamp::parse(TimestampFormat::DateTime, s).ok()?.into();
    Some(t.into())
}

pub(crate) fn parse_time(s: &str) -> Result<SystemTime, ControlError> {
    parse_rfc3339(s).ok_or_else(|| ControlError::BadRequest(format!("invalid time {s:?}, expected RFC 3339 such as 2030-01-02T03:04:05Z")))
}

// ---- JSON views ----

fn clock_json(c: &Clock) -> Value {
    json!({ "now": fmt_time(c.now()), "frozen": c.is_frozen(), "shifted": c.is_virtual() })
}

fn counts_json(c: Counts) -> Value {
    json!({ "buckets": c.buckets, "objects": c.objects, "uploads": c.uploads })
}

fn memory_json(m: crate::memory::MemoryStats) -> Value {
    json!({ "used": m.used, "limit": m.limit })
}

fn snapshot_json(s: &SnapshotInfo) -> Value {
    json!({ "name": s.name, "created": fmt_time(s.created), "buckets": s.counts.buckets, "objects": s.counts.objects, "uploads": s.counts.uploads })
}

fn record_json(r: &RequestRecord) -> Value {
    let (bucket, key) = r.bucket_key();
    json!({
        "seq": r.seq,
        "time": fmt_time(r.time),
        "method": r.method.as_str(),
        "path": r.uri,
        "bucket": bucket,
        "key": key,
        "operation": r.operation(),
        "status": r.status,
        "latency_us": r.latency.as_micros() as u64,
        "bytes_in": r.bytes_in,
        "bytes_out": r.bytes_out,
        "access_key": r.access_key,
    })
}

fn log_config_json(l: &RequestLog) -> Value {
    json!({ "enabled": l.is_enabled(), "capacity": l.capacity(), "stored": l.len(), "dropped": l.dropped() })
}

// ---- HTTP ----

type Response = http::Response<s3s::Body>;

fn reply(status: u16, body: Value) -> Response {
    let bytes = serde_json::to_vec(&body).unwrap_or_default();
    http::Response::builder()
        .status(status)
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(http::header::CONTENT_LENGTH, bytes.len())
        .body(s3s::Body::from(Bytes::from(bytes)))
        .expect("static response parts are valid")
}

fn error(status: u16, code: &str, message: impl fmt::Display) -> Response {
    reply(status, json!({ "error": { "code": code, "message": message.to_string() } }))
}

impl From<ControlError> for Response {
    fn from(e: ControlError) -> Self {
        match e {
            ControlError::BadRequest(m) => error(400, "BadRequest", m),
            ControlError::NotFound(m) => error(404, "NotFound", m),
            ControlError::Conflict(m) => error(409, "Conflict", m),
        }
    }
}

type Query = Vec<(String, String)>;

fn query_flag(q: &Query, name: &str, default: bool) -> Result<bool, ControlError> {
    match q.iter().find(|(k, _)| k == name) {
        None => Ok(default),
        Some((_, v)) => match v.as_str() {
            "" | "true" | "1" => Ok(true),
            "false" | "0" => Ok(false),
            _ => Err(ControlError::BadRequest(format!("{name} must be true or false"))),
        },
    }
}

fn parse_body<T: for<'de> Deserialize<'de> + Default>(body: &[u8]) -> Result<T, ControlError> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(T::default());
    }
    serde_json::from_slice(body).map_err(|e| ControlError::BadRequest(format!("invalid JSON body: {e}")))
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct NameBody {
    name: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct AtBody {
    at: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdvanceBody {
    by: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct LogConfigBody {
    enabled: Option<bool>,
    capacity: Option<usize>,
}

impl Control {
    /// Answers a request under [`PREFIX`].
    pub(crate) async fn handle(&self, req: http::Request<hyper::body::Incoming>) -> Response {
        let (parts, body) = req.into_parts();
        // A web page can make a browser send simple cross-origin POSTs to localhost; refuse them
        // so that visiting a page cannot reset the server. Test tooling never sends `Origin`.
        if parts.method != http::Method::GET && parts.headers.contains_key(http::header::ORIGIN) {
            return error(403, "Forbidden", "cross-origin requests are not allowed");
        }
        let body = match Limited::new(body, MAX_BODY).collect().await {
            Ok(b) => b.to_bytes(),
            Err(_) => return error(413, "TooLarge", "request body too large"),
        };
        let query: Query = form_urlencoded::parse(parts.uri.query().unwrap_or("").as_bytes())
            .into_owned()
            .collect();
        self.route(&parts.method, parts.uri.path(), &query, &body)
            .unwrap_or_else(Response::from)
    }

    fn route(&self, method: &http::Method, path: &str, q: &Query, body: &[u8]) -> Result<Response, ControlError> {
        use http::Method as M;
        let rest = path.strip_prefix(PREFIX).unwrap_or("").trim_end_matches('/');
        let segs: Vec<&str> = rest.split('/').skip(1).collect();
        let not_allowed = || Ok(error(405, "MethodNotAllowed", format!("{method} is not allowed on {path}")));
        match segs.as_slice() {
            [""] | [] => match *method {
                M::GET => Ok(reply(
                    200,
                    json!({
                        "version": env!("CARGO_PKG_VERSION"),
                        "clock": clock_json(&self.clock),
                        "store": counts_json(self.counts()),
                        "memory": memory_json(self.memory.stats()),
                        "bucket_defaults": self.bucket_defaults.as_str(),
                        "auth": if self.auth { "strict" } else { "any-credentials" },
                        "snapshots": self.snapshots.lock().len(),
                        "request_log": log_config_json(&self.log),
                    }),
                )),
                _ => not_allowed(),
            },
            ["stats"] => match *method {
                M::GET => Ok(reply(
                    200,
                    json!({
                        "memory": memory_json(self.memory.stats()),
                        "store": counts_json(self.counts()),
                        "snapshots": self.snapshots.lock().len(),
                        "requests": log_config_json(&self.log),
                    }),
                )),
                _ => not_allowed(),
            },
            ["reset"] => match *method {
                M::POST => {
                    let defaults = ResetOptions::default();
                    self.reset_with(ResetOptions {
                        clock: query_flag(q, "clock", defaults.clock)?,
                        requests: query_flag(q, "requests", defaults.requests)?,
                        snapshots: query_flag(q, "snapshots", defaults.snapshots)?,
                    });
                    Ok(reply(200, json!({ "reset": true })))
                }
                _ => not_allowed(),
            },
            ["snapshots"] => match *method {
                M::GET => Ok(reply(
                    200,
                    json!({ "snapshots": self.snapshots().iter().map(snapshot_json).collect::<Vec<_>>() }),
                )),
                M::POST => {
                    let b: NameBody = parse_body(body)?;
                    Ok(reply(201, snapshot_json(&self.snapshot(b.name.as_deref())?)))
                }
                _ => not_allowed(),
            },
            ["snapshots", name] => {
                let name = decode(name);
                match *method {
                    M::DELETE => {
                        self.delete_snapshot(&name)?;
                        Ok(reply(200, json!({ "deleted": name })))
                    }
                    _ => not_allowed(),
                }
            }
            ["snapshots", name, "restore"] => match *method {
                M::POST => Ok(reply(200, snapshot_json(&self.restore(&decode(name))?))),
                _ => not_allowed(),
            },
            ["clock"] => match *method {
                M::GET => Ok(reply(200, clock_json(&self.clock))),
                _ => not_allowed(),
            },
            ["clock", action] => {
                if *method != M::POST {
                    return not_allowed();
                }
                match *action {
                    "freeze" => {
                        let b: AtBody = parse_body(body)?;
                        let at = b.at.as_deref().map(parse_time).transpose()?;
                        self.clock.freeze(at)?;
                    }
                    "unfreeze" => self.clock.unfreeze()?,
                    "set" => {
                        let b: AtBody = parse_body(body)?;
                        let at = b.at.ok_or_else(|| ControlError::BadRequest("\"at\" is required".into()))?;
                        self.clock.set(parse_time(&at)?)?;
                    }
                    "advance" => {
                        let b: AdvanceBody = parse_body(body)?;
                        let by =
                            b.by.ok_or_else(|| ControlError::BadRequest("\"by\" is required, e.g. \"30d\"".into()))?;
                        self.clock.advance(clock::parse_duration(&by).map_err(ControlError::BadRequest)?)?;
                    }
                    "reset" => self.clock.reset(),
                    _ => return Ok(error(404, "NotFound", format!("no such endpoint: {path}"))),
                }
                Ok(reply(200, clock_json(&self.clock)))
            }
            ["requests"] => match *method {
                M::GET => {
                    let filter = request_filter(q)?;
                    let (rows, total) = self.requests(&filter);
                    Ok(reply(
                        200,
                        json!({
                            "requests": rows.iter().map(record_json).collect::<Vec<_>>(),
                            "total": total,
                            "dropped": self.log.dropped(),
                        }),
                    ))
                }
                M::DELETE => {
                    self.clear_requests();
                    Ok(reply(200, json!({ "cleared": true })))
                }
                _ => not_allowed(),
            },
            ["requests", "config"] => match *method {
                M::GET => Ok(reply(200, log_config_json(&self.log))),
                M::PUT => {
                    let b: LogConfigBody = parse_body(body)?;
                    if b.capacity == Some(0) {
                        return Err(ControlError::BadRequest("capacity must be at least 1".into()));
                    }
                    if let Some(on) = b.enabled {
                        self.set_request_log_enabled(on);
                    }
                    if let Some(c) = b.capacity {
                        self.set_request_log_capacity(c);
                    }
                    Ok(reply(200, log_config_json(&self.log)))
                }
                _ => not_allowed(),
            },
            _ => Ok(error(404, "NotFound", format!("no such endpoint: {path}"))),
        }
    }
}

fn decode(s: &str) -> String {
    percent_encoding::percent_decode_str(s).decode_utf8_lossy().into_owned()
}

fn request_filter(q: &Query) -> Result<RequestFilter, ControlError> {
    let mut f = RequestFilter::default();
    for (k, v) in q {
        let bad = |what: &str| ControlError::BadRequest(format!("invalid {k} {v:?}: {what}"));
        match k.as_str() {
            "bucket" => f.bucket = Some(v.clone()),
            "key_prefix" => f.key_prefix = Some(v.clone()),
            "op" | "operation" => f.operation = Some(v.clone()),
            "method" => f.method = Some(v.to_ascii_uppercase().parse().map_err(|_| bad("not an HTTP method"))?),
            "status" => f.status = Some(v.parse::<StatusFilter>().map_err(ControlError::BadRequest)?),
            "since" => f.since = Some(parse_time(v)?),
            "after" => f.after = Some(v.parse().map_err(|_| bad("expected a sequence number"))?),
            "limit" => f.limit = Some(v.parse().map_err(|_| bad("expected a number"))?),
            _ => return Err(ControlError::BadRequest(format!("unknown query parameter {k:?}"))),
        }
    }
    Ok(f)
}
