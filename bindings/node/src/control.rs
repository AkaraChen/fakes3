//! The control plane's clock, request log and fault injection.

use chrono::{DateTime, Utc};
use napi::bindgen_prelude::*;
use napi_derive::napi;

use crate::{Slot, with_server};

/// The server's clock. Every timestamp the server produces (`Last-Modified`, version times, the
/// request log) reads it. Obtained from `server.clock`.
#[napi(js_name = "ClockControl")]
pub struct ClockControl {
    server: Slot,
}

impl ClockControl {
    pub(crate) fn new(server: Slot) -> Self {
        Self { server }
    }
}

fn clock_error(e: fakes3::OutOfRange) -> Error {
    Error::new(Status::InvalidArg, e.to_string())
}

#[napi]
impl ClockControl {
    /// The current server time.
    #[napi]
    pub fn now(&self) -> Result<DateTime<Utc>> {
        with_server(&self.server, |s| s.clock().now().into())
    }

    /// Whether time is stopped.
    #[napi]
    pub fn is_frozen(&self) -> Result<bool> {
        with_server(&self.server, |s| s.clock().is_frozen())
    }

    /// Whether the clock differs from the host clock (frozen, or running with an offset).
    #[napi]
    pub fn is_shifted(&self) -> Result<bool> {
        with_server(&self.server, |s| s.clock().is_virtual())
    }

    /// Stops time at `at`, or at the current server time when omitted.
    #[napi]
    pub fn freeze(&self, at: Option<DateTime<Utc>>) -> Result<()> {
        with_server(&self.server, |s| s.clock().freeze(at.map(Into::into)))?.map_err(clock_error)
    }

    /// Lets time run again from the current server time.
    #[napi]
    pub fn unfreeze(&self) -> Result<()> {
        with_server(&self.server, |s| s.clock().unfreeze())?.map_err(clock_error)
    }

    /// Jumps to `at`, keeping the frozen or running state.
    #[napi]
    pub fn set(&self, at: DateTime<Utc>) -> Result<()> {
        with_server(&self.server, |s| s.clock().set(at.into()))?.map_err(clock_error)
    }

    /// Moves time forward, or backward when negative. `by` is a number of milliseconds or a string
    /// such as `"30d"`, `"1h30m"`, `"500ms"` or `"-2w"` (units `ns us ms s m h d w`; a bare number
    /// in a string is seconds).
    #[napi]
    pub fn advance(&self, by: Either<f64, String>) -> Result<()> {
        let nanos = match by {
            Either::A(ms) if ms.is_finite() => (ms * 1e6) as i64,
            Either::A(ms) => return Err(Error::new(Status::InvalidArg, format!("invalid duration: {ms}"))),
            Either::B(s) => fakes3::parse_duration(&s).map_err(|e| Error::new(Status::InvalidArg, e))?,
        };
        with_server(&self.server, |s| s.clock().advance(nanos))?.map_err(clock_error)
    }

    /// Puts the clock back on the host clock.
    #[napi]
    pub fn reset(&self) -> Result<()> {
        with_server(&self.server, |s| s.clock().reset())
    }
}

/// Which logged requests to return. Unset fields match everything.
#[napi(object)]
pub struct RequestFilter {
    pub bucket: Option<String>,
    /// Only keys starting with this.
    pub key_prefix: Option<String>,
    /// S3 operation name such as `PutObject` (case-insensitive).
    pub operation: Option<String>,
    /// HTTP method such as `PUT`.
    pub method: Option<String>,
    /// An exact status (`404`), a class (`"4xx"`), or `"error"` for 400 and above.
    pub status: Option<Either<u32, String>>,
    /// Only requests at or after this server time.
    pub since: Option<DateTime<Utc>>,
    /// Only requests with a `seq` greater than this.
    pub after: Option<f64>,
    /// Keep only the most recent `limit` matches.
    pub limit: Option<u32>,
}

/// One logged S3 request.
#[napi(object)]
pub struct RequestRecord {
    /// Position in arrival order, starting at 1; keeps counting across `clear()` and `reset()`.
    pub seq: f64,
    /// Server clock time when the request arrived.
    pub time: DateTime<Utc>,
    pub method: String,
    /// Path and query exactly as sent.
    pub path: String,
    pub bucket: Option<String>,
    pub key: Option<String>,
    /// S3 operation name such as `PutObject`.
    pub operation: String,
    pub status: u32,
    /// From arrival until the response head was ready.
    pub latency_us: f64,
    pub bytes_in: f64,
    pub bytes_out: f64,
    /// The access key the request was signed with. `null` for unsigned requests, and with
    /// `auth: true`, where it is always the configured key.
    pub access_key: Option<String>,
    /// The `x-amz-request-id` the response carried.
    pub request_id: String,
    /// Set when a fault rule hit this request. `status` is 0 for a reset connection.
    pub fault: Option<InjectedFault>,
}

/// Settings for the request log.
#[napi(object)]
pub struct RequestLogOptions {
    /// Turn recording on or off.
    pub enabled: Option<bool>,
    /// Requests kept; the oldest are dropped first.
    pub capacity: Option<u32>,
}

fn to_filter(f: Option<RequestFilter>) -> Result<fakes3::RequestFilter> {
    let Some(f) = f else {
        return Ok(fakes3::RequestFilter::default());
    };
    let bad = |m: String| Error::new(Status::InvalidArg, m);
    let status = match f.status {
        None => None,
        Some(Either::A(n)) => Some(fakes3::StatusFilter::Exact(
            u16::try_from(n).map_err(|_| bad(format!("invalid status {n}")))?,
        )),
        Some(Either::B(s)) => Some(s.parse().map_err(bad)?),
    };
    let method = match f.method {
        None => None,
        Some(m) => Some(http::Method::from_bytes(m.to_ascii_uppercase().as_bytes()).map_err(|_| bad(format!("invalid method {m:?}")))?),
    };
    Ok(fakes3::RequestFilter {
        bucket: f.bucket,
        key_prefix: f.key_prefix,
        operation: f.operation,
        method,
        status,
        since: f.since.map(Into::into),
        after: f.after.map(|n| n as u64),
        limit: f.limit.map(|n| n as usize),
    })
}

fn record(r: fakes3::RequestRecord) -> RequestRecord {
    let (bucket, key) = r.bucket_key();
    let request_id = r.request_id();
    RequestRecord {
        seq: r.seq as f64,
        time: r.time.into(),
        method: r.method.to_string(),
        operation: r.operation().to_string(),
        path: r.uri,
        bucket,
        key,
        status: r.status.into(),
        latency_us: r.latency.as_micros() as f64,
        bytes_in: r.bytes_in as f64,
        bytes_out: r.bytes_out as f64,
        request_id,
        access_key: r.access_key,
        fault: r.fault.map(|f| InjectedFault {
            id: f.id as f64,
            kind: f.kind.as_str().to_owned(),
            delayed: f.delayed,
        }),
    }
}

/// The log of S3 requests the server has received (the control plane's own requests are not
/// logged). Obtained from `server.requests`.
#[napi(js_name = "RequestsControl")]
pub struct RequestsControl {
    server: Slot,
}

impl RequestsControl {
    pub(crate) fn new(server: Slot) -> Self {
        Self { server }
    }
}

#[napi]
impl RequestsControl {
    /// Logged requests matching `filter`, oldest first.
    #[napi]
    pub fn list(&self, filter: Option<RequestFilter>) -> Result<Vec<RequestRecord>> {
        let f = to_filter(filter)?;
        Ok(with_server(&self.server, |s| s.requests(&f))?.0.into_iter().map(record).collect())
    }

    /// How many logged requests match `filter`, ignoring its `limit`.
    #[napi]
    pub fn count(&self, filter: Option<RequestFilter>) -> Result<f64> {
        let f = fakes3::RequestFilter {
            limit: None,
            ..to_filter(filter)?
        };
        Ok(with_server(&self.server, |s| s.requests(&f))?.1 as f64)
    }

    /// Empties the log.
    #[napi]
    pub fn clear(&self) -> Result<()> {
        with_server(&self.server, |s| s.control().clear_requests())
    }

    /// Turns recording on or off, and changes how many requests are kept.
    #[napi]
    pub fn configure(&self, options: RequestLogOptions) -> Result<()> {
        with_server(&self.server, |s| {
            if let Some(on) = options.enabled {
                s.control().set_request_log_enabled(on);
            }
            if let Some(n) = options.capacity {
                s.control().set_request_log_capacity(n as usize);
            }
        })
    }
}

/// How long a delayed request waits: milliseconds, a duration string such as `"500ms"` or `"2s"`,
/// or `{ min, max }` for a random delay in that range.
#[napi(object)]
pub struct DelayRange {
    pub min: Either<f64, String>,
    pub max: Either<f64, String>,
}

/// The S3 error a fault answers with. `SlowDown`, `ServiceUnavailable` (503), `InternalError`
/// (500) and `RequestTimeout` (400) need no status; a status of 503 or 500 alone means `SlowDown`
/// or `InternalError`.
#[napi(object)]
pub struct FaultError {
    pub status: Option<u32>,
    pub code: Option<String>,
    /// The `<Message>`; S3's message for the code when omitted.
    pub message: Option<String>,
}

/// A fault rule for `server.faults.add()`. Give `error`, `truncate` or `reset` (at most one),
/// `delay`, or a delay and one of the others.
#[napi(object)]
pub struct FaultRule {
    /// S3 operation name such as `GetObject` (case-insensitive), or several. Default: all.
    pub operation: Option<Either<String, Vec<String>>>,
    /// Exact bucket name.
    pub bucket: Option<String>,
    /// Only keys starting with this (and so only requests that address an object).
    pub key_prefix: Option<String>,
    /// Chance from 0 to 1 that a matching request is hit. Default 1.
    pub probability: Option<f64>,
    /// Seed for the probability draws and random delays; the same seed gives the same faults for
    /// the same sequence of requests. Generated when omitted (and reported back).
    pub seed: Option<f64>,
    /// Hit at most this many requests. `times: 1` is "fail once, then let the retry through".
    pub times: Option<u32>,
    /// Let the first `skip` matching requests through first.
    pub skip: Option<u32>,
    /// Wait before handling the request (or before the error / reset).
    pub delay: Option<Either3<f64, String, DelayRange>>,
    /// Answer with an S3 error: a code such as `"SlowDown"`, or `{ status, code, message }`.
    pub error: Option<Either<String, FaultError>>,
    /// Send only the first this-many bytes of the response body, then close the connection.
    pub truncate: Option<f64>,
    /// Close the connection with a TCP reset instead of answering.
    pub reset: Option<bool>,
}

/// A random delay's bounds in milliseconds.
#[napi(object)]
pub struct DelayBounds {
    pub min: f64,
    pub max: f64,
}

/// A stored fault rule and how often it matched and fired.
#[napi(object)]
pub struct FaultInfo {
    pub id: f64,
    pub operation: Option<Vec<String>>,
    pub bucket: Option<String>,
    pub key_prefix: Option<String>,
    pub probability: f64,
    /// The seed in use, generated if none was given.
    pub seed: f64,
    pub times: Option<f64>,
    pub skip: f64,
    /// Milliseconds, or the bounds of a random delay.
    pub delay: Option<Either<f64, DelayBounds>>,
    pub error: Option<FaultError>,
    pub truncate: Option<f64>,
    pub reset: bool,
    /// Requests that matched the operation, bucket and key prefix.
    pub matched: f64,
    /// Requests the rule hit.
    pub fired: f64,
    /// Hits left; omitted when unlimited.
    pub remaining: Option<f64>,
}

/// The fault a logged request got from a fault rule.
#[napi(object)]
pub struct InjectedFault {
    /// The rule's id.
    pub id: f64,
    /// `"error"`, `"delay"` (only delayed), `"truncate"` or `"reset"`.
    #[napi(js_name = "type")]
    pub kind: String,
    /// The request waited before the fault (or before being handled).
    pub delayed: bool,
}

fn invalid(m: impl std::fmt::Display) -> Error {
    Error::new(Status::InvalidArg, m.to_string())
}

fn duration(d: Either<f64, String>) -> Result<std::time::Duration> {
    let arg = match &d {
        Either::A(ms) => fakes3::DurationArg::Millis(*ms),
        Either::B(s) => fakes3::DurationArg::Text(s),
    };
    fakes3::parse_fault_duration(arg).map_err(invalid)
}

fn whole(n: f64, what: &str) -> Result<u64> {
    if n.is_finite() && n >= 0.0 && n.fract() == 0.0 && n <= 9_007_199_254_740_991.0 {
        Ok(n as u64)
    } else {
        Err(invalid(format!("{what} must be a non-negative integer, not {n}")))
    }
}

fn to_rule(r: FaultRule) -> Result<fakes3::FaultRule> {
    let delay = match r.delay {
        None => None,
        Some(Either3::A(ms)) => Some(fakes3::Delay::Fixed(duration(Either::A(ms))?)),
        Some(Either3::B(s)) => Some(fakes3::Delay::Fixed(duration(Either::B(s))?)),
        Some(Either3::C(range)) => Some(fakes3::Delay::Between(duration(range.min)?, duration(range.max)?)),
    };
    let reset = r.reset.unwrap_or(false);
    if usize::from(r.error.is_some()) + usize::from(r.truncate.is_some()) + usize::from(reset) > 1 {
        return Err(invalid("give only one of error, truncate and reset"));
    }
    let action = if let Some(e) = r.error {
        let (status, code, message) = match e {
            Either::A(code) => (None, Some(code), None),
            Either::B(f) => (f.status, f.code, f.message),
        };
        let status = status
            .map(|s| u16::try_from(s).map_err(|_| invalid(format!("invalid status {s}"))))
            .transpose()?;
        Some(fakes3::FaultAction::error_from(status, code.as_deref(), message).map_err(invalid)?)
    } else if let Some(n) = r.truncate {
        Some(fakes3::FaultAction::Truncate {
            bytes: whole(n, "truncate")?,
        })
    } else if reset {
        Some(fakes3::FaultAction::Reset)
    } else {
        None
    };
    Ok(fakes3::FaultRule {
        operations: match r.operation {
            None => Vec::new(),
            Some(Either::A(op)) => vec![op],
            Some(Either::B(ops)) => ops,
        },
        bucket: r.bucket,
        key_prefix: r.key_prefix,
        probability: r.probability.unwrap_or(1.0),
        seed: r.seed.map(|n| whole(n, "seed")).transpose()?,
        times: r.times.map(u64::from),
        skip: r.skip.map_or(0, u64::from),
        delay,
        action,
    })
}

fn ms(d: std::time::Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn fault_info(f: fakes3::FaultInfo) -> FaultInfo {
    let remaining = f.remaining().map(|n| n as f64);
    let r = f.rule;
    let (error, truncate, reset) = match r.action {
        None => (None, None, false),
        Some(fakes3::FaultAction::Error { status, code, message }) => (
            Some(FaultError {
                status: Some(status.into()),
                code: Some(code),
                message,
            }),
            None,
            false,
        ),
        Some(fakes3::FaultAction::Truncate { bytes }) => (None, Some(bytes as f64), false),
        Some(fakes3::FaultAction::Reset) => (None, None, true),
    };
    FaultInfo {
        id: f.id as f64,
        operation: (!r.operations.is_empty()).then_some(r.operations),
        bucket: r.bucket,
        key_prefix: r.key_prefix,
        probability: r.probability,
        seed: r.seed.unwrap_or(0) as f64,
        times: r.times.map(|n| n as f64),
        skip: r.skip as f64,
        delay: r.delay.map(|d| match d {
            fakes3::Delay::Fixed(d) => Either::A(ms(d)),
            fakes3::Delay::Between(lo, hi) => Either::B(DelayBounds { min: ms(lo), max: ms(hi) }),
        }),
        error,
        truncate,
        reset,
        matched: f.matched as f64,
        fired: f.fired as f64,
        remaining,
    }
}

/// Fault injection: make matching S3 requests fail, wait, lose part of their response body or
/// lose their connection. Rules are checked in the order they were added and the first that fires
/// wins; control-plane calls are never affected. Obtained from `server.faults`.
#[napi(js_name = "FaultsControl")]
pub struct FaultsControl {
    server: Slot,
}

impl FaultsControl {
    pub(crate) fn new(server: Slot) -> Self {
        Self { server }
    }
}

#[napi]
impl FaultsControl {
    /// Adds a rule and returns it with its id.
    #[napi]
    pub fn add(&self, rule: FaultRule) -> Result<FaultInfo> {
        let rule = to_rule(rule)?;
        with_server(&self.server, |s| s.add_fault(rule))?
            .map(fault_info)
            .map_err(crate::control_error)
    }

    /// The rules with their counters, in the order they are checked.
    #[napi]
    pub fn list(&self) -> Result<Vec<FaultInfo>> {
        Ok(with_server(&self.server, |s| s.faults())?.into_iter().map(fault_info).collect())
    }

    /// Removes one rule.
    #[napi]
    pub fn remove(&self, id: f64) -> Result<()> {
        let id = whole(id, "id")?;
        with_server(&self.server, |s| s.remove_fault(id))?.map_err(crate::control_error)
    }

    /// Removes every rule.
    #[napi]
    pub fn clear(&self) -> Result<()> {
        with_server(&self.server, |s| s.clear_faults())
    }
}
