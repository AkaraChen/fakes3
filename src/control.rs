//! The test control plane: reset, snapshots, clock, request log and fault injection, as methods on [`Control`]
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
use crate::faults::{Delay, FaultAction, FaultInfo, FaultRule, Faults};
use crate::memory::MemoryTracker;
use crate::reqlog::{RequestFilter, RequestLog, RequestRecord, StatusFilter};
use crate::snapshot::{Counts, StoreSnapshot};
use crate::store::{Store, Versioning};
use crate::users::{UserId, Users};

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
    pub(crate) users: Arc<Users>,
    snapshots: Mutex<BTreeMap<String, SnapshotEntry>>,
    next_snapshot: AtomicU64,
    pub(crate) faults: Faults,
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
    /// Remove the fault rules.
    pub faults: bool,
}

impl Default for ResetOptions {
    fn default() -> Self {
        Self {
            clock: true,
            requests: true,
            snapshots: false,
            faults: true,
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
    /// The accounts, without their secrets.
    fn users_json(&self) -> Value {
        Value::Array(
            self.users
                .iter()
                .map(|(_, u)| {
                    json!({
                        "name": u.name,
                        "access_key": u.access_key,
                        "account_id": u.account_id,
                        "canonical_id": u.canonical_id,
                    })
                })
                .collect(),
        )
    }

    pub(crate) fn new(
        store: Arc<Store>,
        clock: Arc<Clock>,
        memory: Arc<MemoryTracker>,
        log: RequestLog,
        bucket_defaults: BucketDefaults,
        auth: bool,
        users: Arc<Users>,
    ) -> Self {
        Self {
            store,
            clock,
            memory,
            log,
            bucket_defaults,
            auth,
            users,
            snapshots: Mutex::default(),
            next_snapshot: AtomicU64::new(1),
            faults: Faults::default(),
        }
    }

    /// Clears buckets, objects, uploads, the request log, the clock and the fault rules; keeps
    /// snapshots.
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
        if opts.faults {
            self.faults.clear();
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

    /// Creates an empty bucket owned by the default account, in the state a CreateBucket request
    /// without options leaves it, so the bucket defaults apply. It is for in-process tests whose S3
    /// client cannot create buckets (OpenDAL has no bucket API). A bucket the default account
    /// already has is left as it is.
    pub fn create_bucket(&self, name: &str) -> Result<(), ControlError> {
        if !s3s::path::check_bucket_name(name) {
            return Err(ControlError::BadRequest(format!("invalid bucket name {name:?}")));
        }
        let bucket = self.bucket_defaults.new_bucket(self.clock.now(), UserId::DEFAULT);
        if self.store.add_bucket(name.to_owned(), bucket) {
            Ok(())
        } else {
            Err(ControlError::Conflict(format!("bucket {name:?} belongs to another account")))
        }
    }

    /// Turns versioning on for a bucket, as PutBucketVersioning with `Enabled` does, minus the MFA
    /// Delete check.
    pub fn enable_versioning(&self, bucket: &str) -> Result<(), ControlError> {
        let b = self
            .store
            .bucket(bucket)
            .ok_or_else(|| ControlError::NotFound(format!("no bucket named {bucket:?}")))?;
        *b.versioning.write() = Versioning::Enabled;
        Ok(())
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

    /// Adds a fault rule after the existing ones. See [`FaultRule`].
    pub fn add_fault(&self, rule: FaultRule) -> Result<FaultInfo, ControlError> {
        self.faults.add(rule)
    }

    /// The fault rules in the order they are checked, with their counters.
    pub fn faults(&self) -> Vec<FaultInfo> {
        self.faults.list()
    }

    /// One fault rule.
    pub fn fault(&self, id: u64) -> Result<FaultInfo, ControlError> {
        self.faults.get(id).ok_or_else(|| no_fault(id))
    }

    pub fn remove_fault(&self, id: u64) -> Result<(), ControlError> {
        if self.faults.remove(id) { Ok(()) } else { Err(no_fault(id)) }
    }

    /// Removes every fault rule and returns how many there were.
    pub fn clear_faults(&self) -> usize {
        self.faults.clear()
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

fn no_fault(id: u64) -> ControlError {
    ControlError::NotFound(format!("no fault rule with id {id}"))
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
        "request_id": r.request_id(),
        "fault": r.fault.map(|f| json!({ "id": f.id, "type": f.kind.as_str(), "delayed": f.delayed })),
    })
}

fn ms(d: std::time::Duration) -> Value {
    json!(d.as_secs_f64() * 1000.0)
}

fn fault_json(f: &FaultInfo) -> Value {
    let r = &f.rule;
    let delay = match r.delay {
        None => Value::Null,
        Some(Delay::Fixed(d)) => ms(d),
        Some(Delay::Between(lo, hi)) => json!({ "min": ms(lo), "max": ms(hi) }),
    };
    let (error, truncate, reset) = match &r.action {
        None => (Value::Null, Value::Null, false),
        Some(FaultAction::Error { status, code, message }) => {
            (json!({ "status": status, "code": code, "message": message }), Value::Null, false)
        }
        Some(FaultAction::Truncate { bytes }) => (Value::Null, json!(bytes), false),
        Some(FaultAction::Reset) => (Value::Null, Value::Null, true),
    };
    json!({
        "id": f.id,
        "operation": if r.operations.is_empty() { Value::Null } else { json!(r.operations) },
        "bucket": r.bucket,
        "key_prefix": r.key_prefix,
        "probability": r.probability,
        "seed": r.seed,
        "times": r.times,
        "skip": r.skip,
        "delay": delay,
        "error": error,
        "truncate": truncate,
        "reset": reset,
        "matched": f.matched,
        "fired": f.fired,
        "remaining": f.remaining(),
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

/// `POST /_fakes3/faults`.
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FaultBody {
    operation: Option<OneOrMany>,
    bucket: Option<String>,
    key_prefix: Option<String>,
    probability: Option<f64>,
    seed: Option<u64>,
    times: Option<u64>,
    skip: Option<u64>,
    delay: Option<DelayBody>,
    error: Option<ErrorBody>,
    truncate: Option<u64>,
    reset: Option<bool>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

/// Milliseconds, or a duration string such as `"1.5s"` or `"500ms"`.
#[derive(Deserialize)]
#[serde(untagged)]
enum DurationBody {
    Millis(f64),
    Text(String),
}

#[derive(Deserialize)]
#[serde(untagged)]
enum DelayBody {
    One(DurationBody),
    Range(DelayRange),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DelayRange {
    min: DurationBody,
    max: DurationBody,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ErrorBody {
    Code(String),
    Full(ErrorFields),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ErrorFields {
    status: Option<u16>,
    code: Option<String>,
    message: Option<String>,
}

/// A non-negative duration from milliseconds or a duration string (`"500ms"`, `"2s"`, `"1m"`).
pub fn parse_fault_duration(d: DurationArg<'_>) -> Result<std::time::Duration, ControlError> {
    let bad = |m: String| ControlError::BadRequest(m);
    match d {
        DurationArg::Millis(ms) if ms.is_finite() && ms >= 0.0 => Ok(std::time::Duration::from_secs_f64(ms / 1000.0)),
        DurationArg::Millis(ms) => Err(bad(format!(
            "invalid delay {ms}: use a number of milliseconds or a string such as \"500ms\""
        ))),
        DurationArg::Text(s) => {
            let n = clock::parse_duration(s).map_err(bad)?;
            u64::try_from(n)
                .map(std::time::Duration::from_nanos)
                .map_err(|_| bad(format!("delay {s:?} is negative")))
        }
    }
}

/// A duration as JSON or Node pass it: a number of milliseconds or a string.
#[derive(Clone, Copy, Debug)]
pub enum DurationArg<'a> {
    Millis(f64),
    Text(&'a str),
}

impl DurationBody {
    fn get(&self) -> Result<std::time::Duration, ControlError> {
        parse_fault_duration(match self {
            Self::Millis(n) => DurationArg::Millis(*n),
            Self::Text(s) => DurationArg::Text(s),
        })
    }
}

impl FaultBody {
    fn into_rule(self) -> Result<FaultRule, ControlError> {
        let bad = |m: &str| Err(ControlError::BadRequest(m.into()));
        let delay = match &self.delay {
            None => None,
            Some(DelayBody::One(d)) => Some(Delay::Fixed(d.get()?)),
            Some(DelayBody::Range(r)) => Some(Delay::Between(r.min.get()?, r.max.get()?)),
        };
        let reset = self.reset.unwrap_or(false);
        let actions = usize::from(self.error.is_some()) + usize::from(self.truncate.is_some()) + usize::from(reset);
        if actions > 1 {
            return bad("give only one of error, truncate and reset");
        }
        let action = if let Some(e) = self.error {
            Some(match e {
                ErrorBody::Code(c) => FaultAction::error_from(None, Some(&c), None)?,
                ErrorBody::Full(f) => FaultAction::error_from(f.status, f.code.as_deref(), f.message)?,
            })
        } else if let Some(bytes) = self.truncate {
            Some(FaultAction::Truncate { bytes })
        } else if reset {
            Some(FaultAction::Reset)
        } else {
            None
        };
        Ok(FaultRule {
            operations: match self.operation {
                None => Vec::new(),
                Some(OneOrMany::One(o)) => vec![o],
                Some(OneOrMany::Many(v)) => v,
            },
            bucket: self.bucket,
            key_prefix: self.key_prefix,
            probability: self.probability.unwrap_or(1.0),
            seed: self.seed,
            times: self.times,
            skip: self.skip.unwrap_or(0),
            delay,
            action,
        })
    }
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
                        "users": self.users_json(),
                        "snapshots": self.snapshots.lock().len(),
                        "request_log": log_config_json(&self.log),
                        "faults": self.faults.len(),
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
                        "faults": self.faults.len(),
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
                        faults: query_flag(q, "faults", defaults.faults)?,
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
            ["faults"] => match *method {
                M::GET => Ok(reply(
                    200,
                    json!({ "faults": self.faults().iter().map(fault_json).collect::<Vec<_>>() }),
                )),
                M::POST => {
                    let b: FaultBody = parse_body(body)?;
                    Ok(reply(201, fault_json(&self.add_fault(b.into_rule()?)?)))
                }
                M::DELETE => Ok(reply(200, json!({ "cleared": self.clear_faults() }))),
                _ => not_allowed(),
            },
            ["faults", id] => {
                let id: u64 = id
                    .parse()
                    .map_err(|_| ControlError::NotFound(format!("no fault rule with id {id:?}")))?;
                match *method {
                    M::GET => Ok(reply(200, fault_json(&self.fault(id)?))),
                    M::DELETE => {
                        self.remove_fault(id)?;
                        Ok(reply(200, json!({ "deleted": id })))
                    }
                    _ => not_allowed(),
                }
            }
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
