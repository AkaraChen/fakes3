//! The control plane's clock and request log.

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
        access_key: r.access_key,
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
