//! Request ids: `x-amz-request-id` (16 upper-case hex digits, like S3's) and `x-amz-id-2` on every
//! response, `<RequestId>` / `<HostId>` in error bodies, and the same id in the request log.
//!
//! An id is one `u64`: a process-wide counter pushed through a bijective mixer (splitmix64), so ids
//! are unique, look random and cost one relaxed `fetch_add`, no lock. `x-amz-id-2` is derived from
//! the id, so the log only stores the `u64`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;

use crate::names;

const GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Identifies one request in its response headers, its error body and the request log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RequestId(u64);

impl RequestId {
    pub fn next() -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        // The start offset differs per process, so ids of two runs do not repeat.
        Self(mix(n.wrapping_add(start_offset())))
    }

    /// The value of `x-amz-request-id`.
    pub fn hex(self) -> [u8; 16] {
        const DIGITS: &[u8; 16] = b"0123456789ABCDEF";
        let mut out = [0u8; 16];
        for (i, b) in out.iter_mut().enumerate() {
            *b = DIGITS[((self.0 >> (60 - 4 * i)) & 0xF) as usize];
        }
        out
    }

    pub fn to_string_id(self) -> String {
        self.hex().iter().map(|&b| b as char).collect()
    }

    /// The value of `x-amz-id-2` (S3's extended request id: 64 base64 characters), on the stack.
    fn host_id_bytes(self) -> [u8; 64] {
        let mut raw = [0u8; 48];
        let mut s = self.0;
        for chunk in raw.chunks_exact_mut(8) {
            s = s.wrapping_add(GAMMA);
            chunk.copy_from_slice(&mix(s).to_be_bytes());
        }
        let mut out = [0u8; 64];
        // 48 bytes encode to exactly 64 characters, so the buffer is never short.
        let _ = base64_simd::STANDARD.encode(&raw, base64_simd::Out::from_slice(&mut out));
        out
    }

    #[cfg(test)]
    fn host_id(self) -> String {
        self.host_id_bytes().iter().map(|&b| b as char).collect()
    }

    /// Sets `x-amz-request-id` and `x-amz-id-2` unless the response already has them, and for an
    /// error response writes `<RequestId>` / `<HostId>` into its XML body.
    pub fn decorate(self, resp: &mut s3s::HttpResponse) {
        let host_id = self.host_id_bytes();
        let h = resp.headers_mut();
        set_if_absent(h, names::X_AMZ_REQUEST_ID, &self.hex());
        set_if_absent(h, names::X_AMZ_ID_2, &host_id);
        if resp.status().as_u16() >= 400 {
            self.stamp_error_body(resp, &host_id);
        }
    }

    /// Error bodies are small, single-chunk XML; anything else (HEAD, streams) is left alone.
    fn stamp_error_body(self, resp: &mut s3s::HttpResponse, host_id: &[u8]) {
        let Some(body) = resp.body().bytes() else { return };
        let Some(end) = body.windows(8).rposition(|w| w == b"</Error>") else {
            return;
        };
        let mut out = Vec::with_capacity(body.len() + 128);
        out.extend_from_slice(&body[..end]);
        if !contains(&body, b"<RequestId>") {
            out.extend_from_slice(b"<RequestId>");
            out.extend_from_slice(&self.hex());
            out.extend_from_slice(b"</RequestId>");
        }
        out.extend_from_slice(b"<HostId>");
        out.extend_from_slice(host_id);
        out.extend_from_slice(b"</HostId>");
        out.extend_from_slice(&body[end..]);
        let len = out.len();
        *resp.body_mut() = s3s::Body::from(Bytes::from(out));
        if resp.headers().contains_key(http::header::CONTENT_LENGTH) {
            resp.headers_mut().insert(http::header::CONTENT_LENGTH, len.into());
        }
    }
}

/// One lookup instead of `contains_key` + `insert`.
fn set_if_absent(h: &mut http::HeaderMap, name: http::HeaderName, value: &[u8]) {
    if let http::header::Entry::Vacant(e) = h.entry(name)
        && let Ok(v) = http::HeaderValue::from_bytes(value)
    {
        e.insert(v);
    }
}

pub(crate) fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

fn start_offset() -> u64 {
    static START: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *START.get_or_init(|| {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
        mix(nanos ^ u64::from(std::process::id()).rotate_left(32))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_hex() {
        let a = RequestId::next();
        let b = RequestId::next();
        assert_ne!(a, b);
        let s = a.to_string_id();
        assert_eq!(s.len(), 16);
        assert!(s.bytes().all(|c| c.is_ascii_digit() || (b'A'..=b'F').contains(&c)));
        assert_eq!(a.host_id().len(), 64);
        assert_eq!(a.host_id(), a.host_id());
        assert_ne!(a.host_id(), b.host_id());
    }

    #[test]
    fn error_bodies_get_both_ids() {
        let id = RequestId::next();
        let xml = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Error><Code>NoSuchKey</Code><Message>m</Message></Error>";
        let mut resp = http::Response::builder().status(404).body(s3s::Body::from(xml.to_owned())).unwrap();
        id.decorate(&mut resp);
        let body = String::from_utf8(resp.body().bytes().unwrap().to_vec()).unwrap();
        assert!(
            body.contains(&format!(
                "<RequestId>{}</RequestId><HostId>{}</HostId></Error>",
                id.to_string_id(),
                id.host_id()
            )),
            "{body}"
        );
        assert_eq!(resp.headers()["x-amz-request-id"], id.to_string_id().as_str());
        assert_eq!(resp.headers()["x-amz-id-2"], id.host_id().as_str());
    }
}
