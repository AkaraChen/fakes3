//! Bounded in-memory log of S3 requests, for tests that assert on call counts and order.
//!
//! The hot path only stores what the request already carries (method, URI, a few numbers). The
//! bucket, key and operation name are derived when the log is read, so recording stays cheap.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use parking_lot::Mutex;

use crate::names;
use crate::reqid::RequestId;

/// One recorded request.
#[derive(Clone, Debug)]
pub struct RequestRecord {
    /// Position in the order requests arrived, starting at 1; survives `clear`.
    pub seq: u64,
    /// Server clock time when the request arrived.
    pub time: SystemTime,
    pub method: http::Method,
    /// Path and query exactly as sent.
    pub uri: String,
    pub status: u16,
    /// From arrival until the response head was ready; a streamed body is not included.
    pub latency: Duration,
    /// Request payload size (`x-amz-decoded-content-length` if present, else `Content-Length`).
    pub bytes_in: u64,
    /// Response `Content-Length`, 0 when unknown.
    pub bytes_out: u64,
    /// `x-amz-copy-source` was present.
    copy: bool,
    /// The access key the request was signed with. Only recorded when signatures are not verified
    /// (`--no-auth`), where it can be anything; otherwise it is always the configured key.
    pub access_key: Option<String>,
    id: RequestId,
}

impl RequestRecord {
    /// The `x-amz-request-id` the response carried.
    pub fn request_id(&self) -> String {
        self.id.to_string_id()
    }

    /// Path-style bucket and key, percent-decoded.
    pub fn bucket_key(&self) -> (Option<String>, Option<String>) {
        let path = self.uri.split('?').next().unwrap_or("");
        let path = path.strip_prefix('/').unwrap_or(path);
        if path.is_empty() {
            return (None, None);
        }
        match path.split_once('/') {
            Some((b, k)) if !k.is_empty() => (Some(decode(b)), Some(decode(k))),
            Some((b, _)) => (Some(decode(b)), None),
            None => (Some(decode(path)), None),
        }
    }

    /// The S3 operation, inferred from the request's shape (empty when not recognized).
    pub fn operation(&self) -> &'static str {
        let (bucket, key) = self.bucket_key();
        let query = self.uri.split_once('?').map_or("", |(_, q)| q);
        operation(&self.method, bucket.is_some(), key.is_some(), query, self.copy)
    }
}

fn decode(s: &str) -> String {
    percent_encoding::percent_decode_str(s).decode_utf8_lossy().into_owned()
}

fn has(query: &str, name: &str) -> bool {
    query.split('&').any(|p| p.split('=').next() == Some(name))
}

fn operation(method: &http::Method, bucket: bool, key: bool, query: &str, copy: bool) -> &'static str {
    use http::Method as M;
    let q = |name| has(query, name);
    if !bucket {
        return if method == M::GET { "ListBuckets" } else { "" };
    }
    if !key {
        // Bucket subresources that follow the Get/Put/DeleteBucket<Name> pattern.
        const SUB: &[(&str, [&str; 3])] = &[
            ("tagging", ["GetBucketTagging", "PutBucketTagging", "DeleteBucketTagging"]),
            ("versioning", ["GetBucketVersioning", "PutBucketVersioning", ""]),
            ("acl", ["GetBucketAcl", "PutBucketAcl", ""]),
            ("cors", ["GetBucketCors", "PutBucketCors", "DeleteBucketCors"]),
            ("policy", ["GetBucketPolicy", "PutBucketPolicy", "DeleteBucketPolicy"]),
            (
                "lifecycle",
                [
                    "GetBucketLifecycleConfiguration",
                    "PutBucketLifecycleConfiguration",
                    "DeleteBucketLifecycle",
                ],
            ),
            ("website", ["GetBucketWebsite", "PutBucketWebsite", "DeleteBucketWebsite"]),
            (
                "encryption",
                ["GetBucketEncryption", "PutBucketEncryption", "DeleteBucketEncryption"],
            ),
            ("object-lock", ["GetObjectLockConfiguration", "PutObjectLockConfiguration", ""]),
        ];
        let slot = match *method {
            M::GET => Some(0),
            M::PUT => Some(1),
            M::DELETE => Some(2),
            _ => None,
        };
        if let Some((_, names)) = SUB.iter().find(|(s, _)| q(s)) {
            return slot.map_or("", |i| names[i]);
        }
        return match *method {
            M::HEAD => "HeadBucket",
            M::PUT => "CreateBucket",
            M::DELETE => "DeleteBucket",
            M::POST if q("delete") => "DeleteObjects",
            M::GET if q("location") => "GetBucketLocation",
            M::GET if q("versions") => "ListObjectVersions",
            M::GET if q("uploads") => "ListMultipartUploads",
            M::GET if query.split('&').any(|p| p == "list-type=2") => "ListObjectsV2",
            M::GET => "ListObjects",
            _ => "",
        };
    }
    match *method {
        M::HEAD => "HeadObject",
        M::GET if q("tagging") => "GetObjectTagging",
        M::GET if q("uploadId") => "ListParts",
        M::GET if q("attributes") => "GetObjectAttributes",
        M::GET if q("acl") => "GetObjectAcl",
        M::GET => "GetObject",
        M::PUT if q("tagging") => "PutObjectTagging",
        M::PUT if q("acl") => "PutObjectAcl",
        M::PUT if q("partNumber") && q("uploadId") => {
            if copy {
                "UploadPartCopy"
            } else {
                "UploadPart"
            }
        }
        M::PUT if copy => "CopyObject",
        M::PUT => "PutObject",
        M::DELETE if q("tagging") => "DeleteObjectTagging",
        M::DELETE if q("uploadId") => "AbortMultipartUpload",
        M::DELETE => "DeleteObject",
        M::POST if q("uploads") => "CreateMultipartUpload",
        M::POST if q("uploadId") => "CompleteMultipartUpload",
        M::POST if q("restore") => "RestoreObject",
        M::POST if q("select") => "SelectObjectContent",
        _ => "",
    }
}

/// What to keep when reading the log. Unset fields match everything.
#[derive(Clone, Debug, Default)]
pub struct RequestFilter {
    pub bucket: Option<String>,
    /// Key prefix.
    pub key_prefix: Option<String>,
    pub operation: Option<String>,
    pub method: Option<http::Method>,
    pub status: Option<StatusFilter>,
    /// Only requests at or after this server time.
    pub since: Option<SystemTime>,
    /// Only requests with a `seq` greater than this.
    pub after: Option<u64>,
    /// Keep only the most recent `limit` matches.
    pub limit: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusFilter {
    Exact(u16),
    /// `2xx`, `4xx`, ...
    Class(u16),
    /// 400 and above.
    Error,
}

impl std::str::FromStr for StatusFilter {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        if s == "error" {
            return Ok(Self::Error);
        }
        let bad = || format!("invalid status {s:?} (use 404, 4xx or error)");
        if let Some(c) = s.strip_suffix("xx") {
            return c.parse().ok().filter(|c| (1..=5).contains(c)).map(Self::Class).ok_or_else(bad);
        }
        s.parse().map(Self::Exact).map_err(|_| bad())
    }
}

impl StatusFilter {
    fn matches(self, status: u16) -> bool {
        match self {
            Self::Exact(s) => s == status,
            Self::Class(c) => status / 100 == c,
            Self::Error => status >= 400,
        }
    }
}

impl RequestFilter {
    /// The conditions that need nothing but the numbers a record stores, checked before a record
    /// is turned into a [`RequestRecord`].
    fn matches_cheap(&self, r: &Stored) -> bool {
        !(self.after.is_some_and(|a| r.seq <= a)
            || self.since.is_some_and(|t| r.time < t)
            || self.method.as_ref().is_some_and(|m| *m != r.method)
            || self.status.is_some_and(|s| !s.matches(r.status)))
    }

    fn matches(&self, r: &RequestRecord) -> bool {
        if self.bucket.is_some() || self.key_prefix.is_some() {
            let (bucket, key) = r.bucket_key();
            if self.bucket.is_some() && self.bucket != bucket {
                return false;
            }
            if let Some(p) = &self.key_prefix
                && !key.is_some_and(|k| k.starts_with(p.as_str()))
            {
                return false;
            }
        }
        self.operation.as_ref().is_none_or(|op| op.eq_ignore_ascii_case(r.operation()))
    }
}

pub const DEFAULT_CAPACITY: usize = 10_000;

pub struct RequestLog {
    enabled: AtomicBool,
    next_seq: AtomicU64,
    inner: Mutex<Inner>,
}

struct Inner {
    capacity: usize,
    records: VecDeque<Stored>,
    /// Records pushed out of the buffer because it was full.
    dropped: u64,
}

/// A string kept inline when it fits in `N` bytes (nearly all request targets and access keys do),
/// so that recording a request allocates nothing, and dropping the oldest record frees nothing
/// (that free would run on another thread than the allocation, the allocator's slow path).
enum Text<const N: usize> {
    Inline(u8, [u8; N]),
    Heap(Box<str>),
}

impl<const N: usize> Text<N> {
    fn new(s: &str) -> Self {
        if s.len() > N.min(u8::MAX as usize) {
            return Self::Heap(s.into());
        }
        let mut buf = [0; N];
        buf[..s.len()].copy_from_slice(s.as_bytes());
        Self::Inline(s.len() as u8, buf)
    }

    fn as_str(&self) -> &str {
        match self {
            // Built from a `&str`, so this is valid UTF-8.
            Self::Inline(n, buf) => std::str::from_utf8(&buf[..*n as usize]).unwrap_or_default(),
            Self::Heap(s) => s,
        }
    }
}

type Target = Text<94>;
type AccessKey = Text<30>;

/// A [`RequestRecord`] as the buffer keeps it.
struct Stored {
    seq: u64,
    time: SystemTime,
    method: http::Method,
    target: Target,
    status: u16,
    latency: Duration,
    bytes_in: u64,
    bytes_out: u64,
    copy: bool,
    access_key: Option<AccessKey>,
    id: RequestId,
}

impl Stored {
    fn record(&self) -> RequestRecord {
        RequestRecord {
            seq: self.seq,
            time: self.time,
            method: self.method.clone(),
            uri: self.target.as_str().to_owned(),
            status: self.status,
            latency: self.latency,
            bytes_in: self.bytes_in,
            bytes_out: self.bytes_out,
            copy: self.copy,
            access_key: self.access_key.as_ref().map(|k| k.as_str().to_owned()),
            id: self.id,
        }
    }
}

/// Request data captured on arrival, completed by [`RequestLog::finish`].
pub struct Pending {
    time: SystemTime,
    start: std::time::Instant,
    method: http::Method,
    target: Target,
    bytes_in: u64,
    copy: bool,
    access_key: Option<AccessKey>,
    id: RequestId,
}

impl Pending {
    pub fn set_access_key(&mut self, key: &str) {
        self.access_key = Some(AccessKey::new(key));
    }
}

impl RequestLog {
    pub fn new(enabled: bool, capacity: usize) -> Self {
        Self {
            enabled: AtomicBool::new(enabled),
            next_seq: AtomicU64::new(1),
            inner: Mutex::new(Inner {
                capacity: capacity.max(1),
                records: VecDeque::new(),
                dropped: 0,
            }),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
    }

    pub fn capacity(&self) -> usize {
        self.inner.lock().capacity
    }

    /// Resizes the buffer, dropping the oldest records if it shrinks.
    pub fn set_capacity(&self, capacity: usize) {
        let mut inner = self.inner.lock();
        inner.capacity = capacity.max(1);
        while inner.records.len() > inner.capacity {
            inner.records.pop_front();
            inner.dropped += 1;
        }
    }

    /// Notes an arriving request, or returns `None` when logging is off.
    pub fn start<B>(
        &self,
        req: &http::Request<B>,
        id: RequestId,
        present: names::Present,
        now: impl FnOnce() -> SystemTime,
    ) -> Option<Pending> {
        if !self.is_enabled() {
            return None;
        }
        let h = req.headers();
        let len = |name: &http::HeaderName| h.get(name).and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok());
        Some(Pending {
            time: now(),
            start: std::time::Instant::now(),
            method: req.method().clone(),
            target: Target::new(req.uri().path_and_query().map_or_else(|| req.uri().path(), |p| p.as_str())),
            bytes_in: present
                .decoded_length
                .then(|| len(&names::X_AMZ_DECODED_CONTENT_LENGTH))
                .flatten()
                .or_else(|| len(&http::header::CONTENT_LENGTH))
                .unwrap_or(0),
            copy: present.copy_source,
            access_key: None,
            id,
        })
    }

    pub fn finish<B>(&self, p: Pending, resp: &http::Response<B>) {
        let status = resp.status().as_u16();
        let bytes_out = resp
            .headers()
            .get(http::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let latency = p.start.elapsed();
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        let record = Stored {
            seq,
            time: p.time,
            method: p.method,
            target: p.target,
            status,
            latency,
            bytes_in: p.bytes_in,
            bytes_out,
            copy: p.copy,
            access_key: p.access_key,
            id: p.id,
        };
        let mut inner = self.inner.lock();
        if inner.records.len() >= inner.capacity {
            inner.records.pop_front();
            inner.dropped += 1;
        }
        inner.records.push_back(record);
    }

    /// Matching records, oldest first, plus the number of matches before `limit` was applied.
    pub fn list(&self, filter: &RequestFilter) -> (Vec<RequestRecord>, usize) {
        let inner = self.inner.lock();
        // `seq` is increasing, so `after` can skip straight to the first candidate.
        let start = filter.after.map_or(0, |a| inner.records.partition_point(|r| r.seq <= a));
        let mut matched: Vec<_> = inner
            .records
            .iter()
            .skip(start)
            .filter(|s| filter.matches_cheap(s))
            .map(Stored::record)
            .filter(|r| filter.matches(r))
            .collect();
        let total = matched.len();
        if let Some(limit) = filter.limit
            && total > limit
        {
            matched.drain(..total - limit);
        }
        (matched, total)
    }

    /// Records evicted because the buffer was full since the last `clear`.
    pub fn dropped(&self) -> u64 {
        self.inner.lock().dropped
    }

    pub fn len(&self) -> usize {
        self.inner.lock().records.len()
    }

    pub fn clear(&self) {
        let mut inner = self.inner.lock();
        inner.records.clear();
        inner.dropped = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(method: &str, uri: &str, copy: bool) -> RequestRecord {
        RequestRecord {
            seq: 1,
            time: SystemTime::UNIX_EPOCH,
            method: method.parse().unwrap(),
            uri: uri.into(),
            status: 200,
            latency: Duration::ZERO,
            bytes_in: 0,
            bytes_out: 0,
            copy,
            access_key: None,
            id: RequestId::next(),
        }
    }

    #[test]
    fn operations() {
        let cases = [
            ("GET", "/", false, "ListBuckets"),
            ("PUT", "/b", false, "CreateBucket"),
            ("HEAD", "/b", false, "HeadBucket"),
            ("DELETE", "/b", false, "DeleteBucket"),
            ("GET", "/b?list-type=2&prefix=x", false, "ListObjectsV2"),
            ("GET", "/b?prefix=x", false, "ListObjects"),
            ("GET", "/b?versions", false, "ListObjectVersions"),
            ("GET", "/b?uploads=", false, "ListMultipartUploads"),
            ("PUT", "/b?versioning", false, "PutBucketVersioning"),
            ("POST", "/b?delete", false, "DeleteObjects"),
            ("PUT", "/b/k", false, "PutObject"),
            ("PUT", "/b/k", true, "CopyObject"),
            ("GET", "/b/a/b/c.txt", false, "GetObject"),
            ("GET", "/b/k?versionId=1", false, "GetObject"),
            ("HEAD", "/b/k", false, "HeadObject"),
            ("DELETE", "/b/k", false, "DeleteObject"),
            ("PUT", "/b/k?tagging", false, "PutObjectTagging"),
            ("POST", "/b/k?uploads", false, "CreateMultipartUpload"),
            ("PUT", "/b/k?partNumber=1&uploadId=u", false, "UploadPart"),
            ("PUT", "/b/k?partNumber=1&uploadId=u", true, "UploadPartCopy"),
            ("POST", "/b/k?uploadId=u", false, "CompleteMultipartUpload"),
            ("DELETE", "/b/k?uploadId=u", false, "AbortMultipartUpload"),
            ("GET", "/b/k?uploadId=u", false, "ListParts"),
            ("PATCH", "/b/k", false, ""),
        ];
        for (method, uri, copy, want) in cases {
            assert_eq!(rec(method, uri, copy).operation(), want, "{method} {uri}");
        }
    }

    #[test]
    fn bucket_and_key_are_decoded() {
        let (b, k) = rec("GET", "/my-bucket/a%20b/c%2Fd+e?x=1", false).bucket_key();
        assert_eq!((b.as_deref(), k.as_deref()), (Some("my-bucket"), Some("a b/c/d+e")));
        assert_eq!(rec("GET", "/", false).bucket_key(), (None, None));
        assert_eq!(rec("GET", "/b/", false).bucket_key(), (Some("b".into()), None));
    }

    fn push(log: &RequestLog, method: &str, uri: &str, status: u16) {
        let req = http::Request::builder().method(method).uri(uri).body(()).unwrap();
        let p = log
            .start(&req, RequestId::next(), names::scan(req.headers()), || SystemTime::UNIX_EPOCH)
            .unwrap();
        log.finish(p, &http::Response::builder().status(status).body(()).unwrap());
    }

    #[test]
    fn filters_and_limit() {
        let log = RequestLog::new(true, 100);
        push(&log, "PUT", "/a", 200);
        push(&log, "PUT", "/a/k1", 200);
        push(&log, "GET", "/a/k2", 404);
        push(&log, "GET", "/b/k1", 200);
        let list = |f: RequestFilter| log.list(&f);
        let f = RequestFilter {
            bucket: Some("a".into()),
            ..Default::default()
        };
        assert_eq!(list(f).1, 3);
        let f = RequestFilter {
            operation: Some("getobject".into()),
            status: Some("4xx".parse().unwrap()),
            ..Default::default()
        };
        assert_eq!(list(f).0[0].uri, "/a/k2");
        let f = RequestFilter {
            key_prefix: Some("k1".into()),
            limit: Some(1),
            ..Default::default()
        };
        let (rows, total) = list(f);
        assert_eq!((rows.len(), total, rows[0].uri.as_str()), (1, 2, "/b/k1"));
        let f = RequestFilter {
            after: Some(3),
            ..Default::default()
        };
        assert_eq!(list(f).0.len(), 1);
    }

    #[test]
    fn ring_buffer_drops_oldest() {
        let log = RequestLog::new(true, 3);
        for i in 0..5 {
            push(&log, "GET", &format!("/b/k{i}"), 200);
        }
        let (rows, _) = log.list(&RequestFilter::default());
        assert_eq!(rows.iter().map(|r| r.seq).collect::<Vec<_>>(), [3, 4, 5]);
        assert_eq!(log.dropped(), 2);
        log.clear();
        push(&log, "GET", "/b/k", 200);
        assert_eq!(log.list(&RequestFilter::default()).0[0].seq, 6);
        log.set_enabled(false);
        assert!(
            log.start(&http::Request::new(()), RequestId::next(), Default::default(), || {
                SystemTime::UNIX_EPOCH
            })
            .is_none()
        );
    }
}
