//! Fault injection: rules that make matching S3 requests fail, slow down, lose part of their
//! response body or lose their connection, so tests can exercise a client's retries and timeouts
//! (see docs/control-plane.md).
//!
//! With no rules stored, the request path pays one relaxed atomic load ([`Faults::is_armed`]).
//! With rules, each S3 request takes the rule lock once to find the first rule that fires.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use bytes::Bytes;
use hyper::body::{Body, Frame, SizeHint};
use parking_lot::Mutex;

use crate::control::ControlError;
use crate::host::Domains;
use crate::linger::Abort;

/// The longest delay a rule may ask for.
const MAX_DELAY: Duration = Duration::from_secs(3600);

/// A fault rule: which requests it applies to, how often, and what it does to them.
///
/// Build one with [`FaultRule::error`], [`FaultRule::slow_down`], [`FaultRule::internal_error`],
/// [`FaultRule::delay`], [`FaultRule::delay_between`], [`FaultRule::truncate`] or
/// [`FaultRule::reset`], then narrow it down:
///
/// ```
/// // The first PutObject under `uploads/` gets 503 SlowDown; the retry goes through.
/// let rule = fakes3::FaultRule::slow_down().operation("PutObject").key_prefix("uploads/").times(1);
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct FaultRule {
    /// S3 operation names such as `GetObject` (case-insensitive); empty matches every operation.
    /// The names are the ones the request log reports.
    pub operations: Vec<String>,
    /// Exact bucket name.
    pub bucket: Option<String>,
    /// Key prefix. A rule with a prefix only matches requests that address an object.
    pub key_prefix: Option<String>,
    /// Chance, from 0 to 1, that a matching request is hit.
    pub probability: f64,
    /// Seed of the rule's random numbers (the probability draws and random delays). The same seed
    /// and the same sequence of matching requests give the same faults. Generated when `None`;
    /// the generated seed is reported by [`FaultInfo`].
    pub seed: Option<u64>,
    /// Hit at most this many requests, then let everything through.
    pub times: Option<u64>,
    /// Let the first `skip` matching requests through before the rule starts hitting.
    pub skip: u64,
    /// Wait before handling the request.
    pub delay: Option<Delay>,
    /// What happens to the request (after the delay, if any).
    pub action: Option<FaultAction>,
}

/// How long a delayed request waits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delay {
    Fixed(Duration),
    /// Uniformly distributed between the two bounds (inclusive).
    Between(Duration, Duration),
}

/// What a fault does to a request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FaultAction {
    /// Answer with an S3 error instead of handling the request: the status, the `<Code>` and the
    /// `<Message>` of the XML error body (a standard message for the code when `None`).
    Error {
        status: u16,
        code: String,
        message: Option<String>,
    },
    /// Handle the request, then send only the first `bytes` bytes of the response body and close
    /// the connection. The response headers, including `Content-Length`, are unchanged.
    Truncate { bytes: u64 },
    /// Close the connection with a TCP reset instead of answering. The request is not handled.
    Reset,
}

/// Status codes and messages for the error codes a rule can name without a status.
const KNOWN_ERRORS: &[(&str, u16, &str)] = &[
    ("SlowDown", 503, "Please reduce your request rate."),
    ("ServiceUnavailable", 503, "Please reduce your request rate."),
    ("InternalError", 500, "We encountered an internal error. Please try again."),
    (
        "RequestTimeout",
        400,
        "Your socket connection to the server was not read from or written to within the timeout period.",
    ),
];

impl FaultAction {
    /// An error action from a code, a status, or both: `SlowDown`, `ServiceUnavailable`,
    /// `InternalError` and `RequestTimeout` have known statuses, and a status of 503 or 500 alone
    /// means `SlowDown` or `InternalError`.
    pub fn error_from(status: Option<u16>, code: Option<&str>, message: Option<String>) -> Result<Self, ControlError> {
        let (status, code) = match (status, code) {
            (Some(s), Some(c)) => (s, c.to_owned()),
            (None, Some(c)) => {
                let known = KNOWN_ERRORS.iter().find(|(k, ..)| *k == c).ok_or_else(|| {
                    ControlError::BadRequest(format!(
                        "error code {c:?} needs a status (only SlowDown, ServiceUnavailable, InternalError and RequestTimeout have a default)"
                    ))
                })?;
                (known.1, c.to_owned())
            }
            (Some(503), None) => (503, "SlowDown".to_owned()),
            (Some(500), None) => (500, "InternalError".to_owned()),
            (Some(s), None) => {
                return Err(ControlError::BadRequest(format!(
                    "status {s} needs an error code (only 500 and 503 have a default)"
                )));
            }
            (None, None) => (503, "SlowDown".to_owned()),
        };
        Ok(Self::Error { status, code, message })
    }

    fn kind(&self) -> FaultKind {
        match self {
            Self::Error { .. } => FaultKind::Error,
            Self::Truncate { .. } => FaultKind::Truncate,
            Self::Reset => FaultKind::Reset,
        }
    }
}

impl FaultRule {
    fn with_action(action: Option<FaultAction>, delay: Option<Delay>) -> Self {
        Self {
            operations: Vec::new(),
            bucket: None,
            key_prefix: None,
            probability: 1.0,
            seed: None,
            times: None,
            skip: 0,
            delay,
            action,
        }
    }

    /// An S3 error response with this status and code.
    pub fn error(status: u16, code: &str) -> Self {
        Self::with_action(
            Some(FaultAction::Error {
                status,
                code: code.to_owned(),
                message: None,
            }),
            None,
        )
    }

    /// 503 `SlowDown`, what S3 answers when it throttles.
    pub fn slow_down() -> Self {
        Self::error(503, "SlowDown")
    }

    /// 500 `InternalError`.
    pub fn internal_error() -> Self {
        Self::error(500, "InternalError")
    }

    /// Wait `d`, then handle the request normally.
    pub fn delay(d: Duration) -> Self {
        Self::with_action(None, Some(Delay::Fixed(d)))
    }

    /// Wait a random time between `min` and `max`, then handle the request normally.
    pub fn delay_between(min: Duration, max: Duration) -> Self {
        Self::with_action(None, Some(Delay::Between(min, max)))
    }

    /// Send only the first `bytes` bytes of the response body, then close the connection.
    pub fn truncate(bytes: u64) -> Self {
        Self::with_action(Some(FaultAction::Truncate { bytes }), None)
    }

    /// Reset the connection instead of answering.
    pub fn reset() -> Self {
        Self::with_action(Some(FaultAction::Reset), None)
    }

    /// Only this operation (call again to add more).
    pub fn operation(mut self, op: &str) -> Self {
        self.operations.push(op.to_owned());
        self
    }

    pub fn bucket(mut self, bucket: &str) -> Self {
        self.bucket = Some(bucket.to_owned());
        self
    }

    pub fn key_prefix(mut self, prefix: &str) -> Self {
        self.key_prefix = Some(prefix.to_owned());
        self
    }

    pub fn probability(mut self, p: f64) -> Self {
        self.probability = p;
        self
    }

    pub fn seed(mut self, seed: u64) -> Self {
        self.seed = Some(seed);
        self
    }

    pub fn times(mut self, n: u64) -> Self {
        self.times = Some(n);
        self
    }

    pub fn skip(mut self, n: u64) -> Self {
        self.skip = n;
        self
    }

    /// Also wait before the action.
    pub fn with_delay(mut self, delay: Delay) -> Self {
        self.delay = Some(delay);
        self
    }

    fn validate(&self) -> Result<(), ControlError> {
        let bad = |m: String| Err(ControlError::BadRequest(m));
        if !(0.0..=1.0).contains(&self.probability) {
            return bad(format!("probability must be between 0 and 1, not {}", self.probability));
        }
        if self.times == Some(0) {
            return bad("times must be at least 1".into());
        }
        if self.delay.is_none() && self.action.is_none() {
            return bad("a fault needs an error, a delay, truncate or reset".into());
        }
        if self.operations.iter().any(String::is_empty) {
            return bad("operation names cannot be empty".into());
        }
        match self.delay {
            Some(Delay::Fixed(d)) if d > MAX_DELAY => return bad("delays are at most 1h".into()),
            Some(Delay::Between(lo, hi)) if lo > hi => return bad("the delay's min is larger than its max".into()),
            Some(Delay::Between(_, hi)) if hi > MAX_DELAY => return bad("delays are at most 1h".into()),
            _ => {}
        }
        if let Some(FaultAction::Error { status, code, .. }) = &self.action {
            if !(400..=599).contains(status) {
                return bad(format!("error status must be 400 to 599, not {status}"));
            }
            if code.is_empty() || !code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_') {
                return bad(format!("invalid error code {code:?}: use letters and digits, such as SlowDown"));
            }
        }
        Ok(())
    }
}

/// A stored rule, as listed.
#[derive(Clone, Debug, PartialEq)]
pub struct FaultInfo {
    pub id: u64,
    /// The rule as added, with `seed` set to the seed in use.
    pub rule: FaultRule,
    /// Requests that matched the rule's operation, bucket and key prefix.
    pub matched: u64,
    /// Requests the rule hit.
    pub fired: u64,
}

impl FaultInfo {
    /// How many more requests the rule will hit; `None` when unlimited.
    pub fn remaining(&self) -> Option<u64> {
        self.rule.times.map(|t| t.saturating_sub(self.fired))
    }
}

/// What kind of fault a logged request got.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultKind {
    Error,
    /// Only delayed; the request was then handled normally.
    Delay,
    Truncate,
    Reset,
}

impl FaultKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Delay => "delay",
            Self::Truncate => "truncate",
            Self::Reset => "reset",
        }
    }
}

/// The fault a request got, as the request log records it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InjectedFault {
    /// The rule's id.
    pub id: u64,
    pub kind: FaultKind,
    /// The request waited first.
    pub delayed: bool,
}

/// The rules of one server.
#[derive(Default)]
pub struct Faults {
    /// Whether any rule is stored; the only thing the request path reads when none is.
    armed: AtomicBool,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    rules: Vec<Rule>,
    next_id: u64,
}

struct Rule {
    id: u64,
    spec: FaultRule,
    rng: SplitMix,
    matched: u64,
    fired: u64,
}

impl Rule {
    fn info(&self) -> FaultInfo {
        FaultInfo {
            id: self.id,
            rule: self.spec.clone(),
            matched: self.matched,
            fired: self.fired,
        }
    }

    fn matches(&self, t: &Target<'_>) -> bool {
        if let Some(b) = &self.spec.bucket
            && t.bucket.as_deref() != Some(b.as_str())
        {
            return false;
        }
        if let Some(p) = &self.spec.key_prefix
            && !t.key.as_deref().is_some_and(|k| k.starts_with(p.as_str()))
        {
            return false;
        }
        self.spec.operations.is_empty() || self.spec.operations.iter().any(|o| o.eq_ignore_ascii_case(t.operation))
    }
}

/// splitmix64: tiny, seedable and good enough for coin flips.
struct SplitMix(u64);

impl SplitMix {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// What a request addresses, worked out only when rules exist.
struct Target<'a> {
    bucket: Option<String>,
    key: Option<String>,
    operation: &'a str,
}

/// A fault chosen for one request.
pub(crate) struct Fired {
    pub injected: InjectedFault,
    pub delay: Option<Duration>,
    pub action: Option<FaultAction>,
    /// For [`FaultAction::Reset`]: resets the connection when it is dropped.
    pub abort: Option<Abort>,
}

impl Faults {
    #[inline]
    pub fn is_armed(&self) -> bool {
        self.armed.load(Ordering::Relaxed)
    }

    pub fn add(&self, mut rule: FaultRule) -> Result<FaultInfo, ControlError> {
        rule.validate()?;
        // Generated seeds stay below 2^53 so that JavaScript reads them back exactly.
        let seed = *rule
            .seed
            .get_or_insert_with(|| uuid::Uuid::new_v4().as_u64_pair().0 & ((1 << 53) - 1));
        let mut inner = self.inner.lock();
        inner.next_id += 1;
        let r = Rule {
            id: inner.next_id,
            spec: rule,
            rng: SplitMix(seed),
            matched: 0,
            fired: 0,
        };
        let info = r.info();
        inner.rules.push(r);
        self.armed.store(true, Ordering::Relaxed);
        Ok(info)
    }

    pub fn list(&self) -> Vec<FaultInfo> {
        self.inner.lock().rules.iter().map(Rule::info).collect()
    }

    pub fn get(&self, id: u64) -> Option<FaultInfo> {
        self.inner.lock().rules.iter().find(|r| r.id == id).map(Rule::info)
    }

    pub fn len(&self) -> usize {
        self.inner.lock().rules.len()
    }

    pub fn remove(&self, id: u64) -> bool {
        let mut inner = self.inner.lock();
        let before = inner.rules.len();
        inner.rules.retain(|r| r.id != id);
        self.armed.store(!inner.rules.is_empty(), Ordering::Relaxed);
        inner.rules.len() != before
    }

    /// Removes every rule and returns how many there were. Ids keep counting.
    pub fn clear(&self) -> usize {
        let mut inner = self.inner.lock();
        let n = inner.rules.len();
        inner.rules.clear();
        self.armed.store(false, Ordering::Relaxed);
        n
    }

    /// The first rule that fires for this request, in the order the rules were added. Only rules
    /// up to that one count the request as matched.
    pub(crate) fn pick<B>(&self, req: &http::Request<B>, domains: &Domains, abort: &Abort) -> Option<Fired> {
        let path = req.uri().path();
        let query = req.uri().query().unwrap_or("");
        let host = req
            .headers()
            .get(http::header::HOST)
            .and_then(|v| v.to_str().ok())
            .or_else(|| req.uri().authority().map(|a| a.as_str()));
        let (bucket, key) = target(domains, host, path);
        let copy = req.headers().contains_key("x-amz-copy-source");
        let t = Target {
            operation: crate::reqlog::operation(req.method(), bucket.is_some(), key.is_some(), query, copy),
            bucket,
            key,
        };
        let mut inner = self.inner.lock();
        for r in inner.rules.iter_mut() {
            if !r.matches(&t) {
                continue;
            }
            r.matched += 1;
            if r.matched <= r.spec.skip || r.spec.times.is_some_and(|n| r.fired >= n) {
                continue;
            }
            if r.spec.probability < 1.0 && r.rng.unit() >= r.spec.probability {
                continue;
            }
            r.fired += 1;
            let delay = match r.spec.delay {
                None => None,
                Some(Delay::Fixed(d)) => Some(d),
                Some(Delay::Between(lo, hi)) => Some(lo + (hi - lo).mul_f64(r.rng.unit())),
            };
            let action = r.spec.action.clone();
            return Some(Fired {
                injected: InjectedFault {
                    id: r.id,
                    kind: action.as_ref().map_or(FaultKind::Delay, FaultAction::kind),
                    delayed: delay.is_some(),
                },
                abort: matches!(action, Some(FaultAction::Reset)).then(|| abort.clone()),
                delay,
                action,
            });
        }
        None
    }
}

/// The bucket (from the host in virtual-host style, else the first path segment) and the key,
/// both percent-decoded.
fn target(domains: &Domains, host: Option<&str>, path: &str) -> (Option<String>, Option<String>) {
    let path = path.strip_prefix('/').unwrap_or(path);
    let decode = |s: &str| percent_encoding::percent_decode_str(s).decode_utf8_lossy().into_owned();
    if let Some(b) = host.and_then(|h| domains.host_bucket(h)) {
        return (Some(b.to_owned()), (!path.is_empty()).then(|| decode(path)));
    }
    if path.is_empty() {
        return (None, None);
    }
    match path.split_once('/') {
        Some((b, k)) if !k.is_empty() => (Some(decode(b)), Some(decode(k))),
        Some((b, _)) => (Some(decode(b)), None),
        None => (Some(decode(path)), None),
    }
}

/// The S3 error response for an [`FaultAction::Error`]. Request ids are added later, like for
/// every other response.
pub(crate) fn error_response(status: u16, code: &str, message: Option<&str>, resource: &str, head: bool) -> s3s::HttpResponse {
    let message = message.unwrap_or_else(|| {
        KNOWN_ERRORS
            .iter()
            .find(|(c, ..)| *c == code)
            .map_or("Fault injected by fakes3.", |k| k.2)
    });
    let status = http::StatusCode::from_u16(status).unwrap_or(http::StatusCode::SERVICE_UNAVAILABLE);
    let builder = http::Response::builder().status(status);
    if head {
        // Like S3, a HEAD error has no body.
        return builder.body(s3s::Body::empty()).expect("static response parts are valid");
    }
    let body = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Error><Code>{}</Code><Message>{}</Message><Resource>{}</Resource></Error>",
        escape(code),
        escape(message),
        escape(resource)
    );
    builder
        .header(http::header::CONTENT_TYPE, "application/xml")
        .header(http::header::CONTENT_LENGTH, body.len())
        .body(s3s::Body::from(body))
        .expect("static response parts are valid")
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// A response body cut after `left` bytes: it then fails, which makes the server close the
/// connection before the length it announced was sent.
pub(crate) struct Truncated {
    inner: s3s::Body,
    left: u64,
    /// Data was dropped.
    cut: bool,
    /// The kept bytes were handed over and the server had a chance to send them.
    flushed: bool,
}

impl Truncated {
    pub fn wrap(body: s3s::Body, bytes: u64) -> s3s::Body {
        s3s::Body::http_body(Self {
            inner: body,
            left: bytes,
            cut: false,
            flushed: false,
        })
    }
}

#[derive(Debug)]
struct Cut;

impl std::fmt::Display for Cut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("response body truncated by fault injection")
    }
}

impl std::error::Error for Cut {}

type StdError = Box<dyn std::error::Error + Send + Sync + 'static>;

impl Body for Truncated {
    type Data = Bytes;
    type Error = StdError;

    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<Frame<Bytes>, StdError>>> {
        use std::task::Poll;
        let this = &mut *self;
        if this.left == 0 {
            if !this.cut && this.inner.is_end_stream() {
                return Poll::Ready(None);
            }
            // The server drops what it has buffered when a body fails, so first let it write out
            // the head and the kept bytes: being pending makes it flush, and it polls again.
            if !this.flushed {
                this.flushed = true;
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }
            return Poll::Ready(Some(Err(Box::new(Cut))));
        }
        match std::pin::Pin::new(&mut this.inner).poll_frame(cx) {
            Poll::Ready(Some(Ok(frame))) => match frame.into_data() {
                Ok(mut data) => {
                    if data.len() as u64 > this.left {
                        data.truncate(this.left as usize);
                        this.cut = true;
                    }
                    this.left -= data.len() as u64;
                    Poll::Ready(Some(Ok(Frame::data(data))))
                }
                Err(frame) => Poll::Ready(Some(Ok(frame))),
            },
            other => other,
        }
    }

    fn size_hint(&self) -> SizeHint {
        // hyper takes the length from the `Content-Length` header; this only matters without one.
        SizeHint::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(method: &str, uri: &str) -> http::Request<()> {
        http::Request::builder().method(method).uri(uri).body(()).unwrap()
    }

    #[test]
    fn unarmed_until_a_rule_is_added() {
        let f = Faults::default();
        assert!(!f.is_armed());
        let info = f.add(FaultRule::slow_down().times(1)).unwrap();
        assert!(f.is_armed());
        assert!(info.rule.seed.is_some());
        assert!(f.remove(info.id));
        assert!(!f.is_armed());
        f.add(FaultRule::reset()).unwrap();
        assert_eq!(f.clear(), 1);
        assert!(!f.is_armed());
    }

    #[test]
    fn matching_skip_and_times() {
        let f = Faults::default();
        let d = Domains::new(["localhost"]);
        let abort = Abort::default();
        f.add(
            FaultRule::internal_error()
                .operation("getobject")
                .bucket("b")
                .key_prefix("x/")
                .skip(1)
                .times(2),
        )
        .unwrap();
        let hit = |m, u| f.pick(&req(m, u), &d, &abort).is_some();
        assert!(!hit("PUT", "/b/x/1"));
        assert!(!hit("GET", "/c/x/1"));
        assert!(!hit("GET", "/b/y/1"));
        assert!(!hit("GET", "/b/x/1")); // skipped
        assert!(hit("GET", "/b/x/2"));
        assert!(hit("GET", "/b/x/3"));
        assert!(!hit("GET", "/b/x/4")); // used up
        let info = &f.list()[0];
        assert_eq!((info.matched, info.fired, info.remaining()), (4, 2, Some(0)));
    }

    #[test]
    fn virtual_host_bucket() {
        let f = Faults::default();
        let d = Domains::new(["localhost"]);
        f.add(FaultRule::slow_down().bucket("vb").key_prefix("k")).unwrap();
        let r = http::Request::builder()
            .method("GET")
            .uri("/k1")
            .header("host", "vb.localhost:9000")
            .body(())
            .unwrap();
        assert!(f.pick(&r, &d, &Abort::default()).is_some());
    }

    #[test]
    fn seeded_probability_repeats() {
        let run = || {
            let f = Faults::default();
            f.add(FaultRule::slow_down().probability(0.5).seed(7)).unwrap();
            let (d, a) = (Domains::new(["localhost"]), Abort::default());
            (0..64).map(|_| f.pick(&req("GET", "/b/k"), &d, &a).is_some()).collect::<Vec<_>>()
        };
        let a = run();
        assert_eq!(a, run());
        let hits = a.iter().filter(|h| **h).count();
        assert!((16..48).contains(&hits), "{hits}");
    }

    #[test]
    fn random_delay_within_bounds() {
        let f = Faults::default();
        let (lo, hi) = (Duration::from_millis(10), Duration::from_millis(20));
        f.add(FaultRule::delay_between(lo, hi).seed(1)).unwrap();
        let (d, a) = (Domains::new(["localhost"]), Abort::default());
        for _ in 0..50 {
            let got = f.pick(&req("GET", "/b/k"), &d, &a).unwrap().delay.unwrap();
            assert!(got >= lo && got <= hi);
        }
    }

    #[test]
    fn validation() {
        assert!(Faults::default().add(FaultRule::slow_down().probability(1.5)).is_err());
        assert!(Faults::default().add(FaultRule::error(200, "Nope")).is_err());
        assert!(Faults::default().add(FaultRule::error(503, "bad code")).is_err());
        assert!(Faults::default().add(FaultRule::slow_down().times(0)).is_err());
        let mut none = FaultRule::slow_down();
        none.action = None;
        assert!(Faults::default().add(none).is_err());
        assert_eq!(
            FaultAction::error_from(None, Some("InternalError"), None).unwrap(),
            FaultAction::Error {
                status: 500,
                code: "InternalError".into(),
                message: None
            }
        );
        assert!(FaultAction::error_from(Some(404), None, None).is_err());
        assert!(FaultAction::error_from(None, Some("NoSuchKey"), None).is_err());
    }

    #[test]
    fn error_body_is_s3_xml() {
        let r = error_response(503, "SlowDown", None, "/b/<k>", false);
        let body = String::from_utf8(r.body().bytes().unwrap().to_vec()).unwrap();
        assert!(body.contains("<Code>SlowDown</Code><Message>Please reduce your request rate.</Message><Resource>/b/&lt;k&gt;</Resource>"));
        assert_eq!(r.status(), 503);
    }
}
