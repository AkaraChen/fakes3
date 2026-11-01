//! Default mode (no `--auth`): accept requests signed with any credentials.
//!
//! s3s has no switch for this. Without an auth provider it answers every signed request with 501,
//! and with one it checks the signature against that provider's secret, which cannot match a
//! secret fakes3 has never seen (`S3Auth` only maps an access key to a secret; it sees nothing of
//! the request). Its aws-chunked decoder is only reachable behind that signature check and
//! verifies every chunk signature, so it cannot be borrowed either.
//!
//! So the service wrapper takes the signature material off the request before s3s sees it. A
//! request without any is anonymous, which s3s accepts without consulting its auth provider:
//!
//! * the `Authorization` header (SigV4, SigV2 and SigV4a alike);
//! * the query parameters of presigned URLs (SigV4 `X-Amz-*`, SigV2 `Signature` and friends);
//! * the aws-chunked framing of `STREAMING-*` uploads, decoded here with the chunk and trailer
//!   signatures ignored. The decoded body goes to s3s with a plain `Content-Length`, and the
//!   trailing headers (checksums) are handed to the S3 implementation through [`Trailers`], so a
//!   trailing checksum is still verified against the data.
//!
//! s3s can no longer tell a signed request from an anonymous one afterwards, so the wrapper records
//! that a signature was there, which access key it named and which identity that is
//! ([`crate::users::Users::resolve`]: an access key nobody configured is the default identity,
//! the owner), in a [`Signed`] request extension. [`crate::access`] turns it into the identity the
//! request acts as.
//!
//! What is not checked any more is the signature itself and everything derived from it: clock
//! skew, the payload hash in `x-amz-content-sha256`, signed-header lists. Signed POST forms
//! (browser uploads with a `signature` field) are not handled and still get 501.

use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::{Bytes, BytesMut};
use futures::Stream;
use parking_lot::Mutex;
use s3s::StdError;
use s3s::stream::{ByteStream, RemainingLength};

use crate::users::{UserId, Users};

/// Query parameters that only exist to authenticate a presigned URL.
const V4_QUERY: &[&str] = &[
    "X-Amz-Algorithm",
    "X-Amz-Credential",
    "X-Amz-Date",
    "X-Amz-Expires",
    "X-Amz-SignedHeaders",
    "X-Amz-Signature",
];
const V2_QUERY: &[&str] = &["AWSAccessKeyId", "Expires", "Signature"];

/// Set on a request whose signature [`accept_any`] removed without checking it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signed {
    /// The access key the signature named, if it could be read.
    pub access_key: Option<String>,
    /// The signature was in the query string (a presigned URL) rather than a header.
    pub presigned: bool,
    /// The identity the access key belongs to.
    pub user: UserId,
}

/// Strips the signature from `req` and decodes an aws-chunked body, so s3s treats it as an
/// ordinary request. A request that carried a signature gets a [`Signed`] extension.
pub fn accept_any<'a>(req: &'a mut http::Request<s3s::Body>, users: &Users) -> Option<&'a Signed> {
    let mut signed = None;
    if let Some(auth) = req.headers_mut().remove(http::header::AUTHORIZATION) {
        signed = Some(Signed {
            access_key: auth.to_str().ok().and_then(header_access_key),
            presigned: false,
            user: UserId::DEFAULT,
        });
    }
    if let Some(query) = req.uri().query()
        && let Some((rest, key)) = strip_presigned(query)
    {
        signed = match signed {
            Some(s) => Some(Signed {
                access_key: s.access_key.or(key),
                ..s
            }),
            None => Some(Signed {
                access_key: key,
                presigned: true,
                user: UserId::DEFAULT,
            }),
        };
        let path = req.uri().path();
        let target = if rest.is_empty() {
            path.to_owned()
        } else {
            format!("{path}?{rest}")
        };
        let mut parts = std::mem::take(req.uri_mut()).into_parts();
        if let Ok(pq) = target.parse() {
            parts.path_and_query = Some(pq);
        }
        if let Ok(uri) = http::Uri::from_parts(parts) {
            *req.uri_mut() = uri;
        }
    }
    let streaming = req
        .headers()
        .get(&crate::names::X_AMZ_CONTENT_SHA256)
        .is_some_and(|v| v.as_bytes().starts_with(b"STREAMING-"));
    if streaming {
        decode_aws_chunked(req);
    }
    let mut signed = signed?;
    signed.user = users.resolve(signed.access_key.as_deref());
    req.extensions_mut().insert(signed);
    req.extensions().get()
}

/// `AWS4-HMAC-SHA256 Credential=<key>/<scope>, ...` or `AWS <key>:<signature>`.
fn header_access_key(auth: &str) -> Option<String> {
    let key = match auth.split_once("Credential=") {
        Some((_, rest)) => rest.split('/').next()?,
        None => auth.strip_prefix("AWS ")?.split(':').next()?,
    };
    Some(key.trim().to_owned()).filter(|k| !k.is_empty())
}

/// For a presigned URL's query, the query without its authentication parameters and the access
/// key it names. `None` when the query is not a presigned URL's.
fn strip_presigned(query: &str) -> Option<(String, Option<String>)> {
    let name = |pair: &str| pair.split('=').next().unwrap_or("").to_owned();
    let has = |n: &str| query.split('&').any(|p| name(p) == n);
    let (drop, key_param) = if has("X-Amz-Signature") {
        (V4_QUERY, "X-Amz-Credential")
    } else if has("Signature") {
        (V2_QUERY, "AWSAccessKeyId")
    } else {
        return None;
    };
    let mut key = None;
    let mut rest = Vec::new();
    for pair in query.split('&') {
        let n = name(pair);
        if n == key_param {
            let value = pair.split_once('=').map_or("", |(_, v)| v);
            let value = percent_encoding::percent_decode_str(value).decode_utf8_lossy();
            key = value.split('/').next().map(str::to_owned).filter(|k| !k.is_empty());
        }
        if !drop.contains(&n.as_str()) {
            rest.push(pair);
        }
    }
    Some((rest.join("&"), key))
}

/// Replaces the body with its decoded payload and `Content-Length` with the decoded length.
fn decode_aws_chunked(req: &mut http::Request<s3s::Body>) {
    let declared = req
        .headers()
        .get(&crate::names::X_AMZ_DECODED_CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    match declared {
        Some(n) => {
            req.headers_mut().insert(http::header::CONTENT_LENGTH, http::HeaderValue::from(n));
        }
        None => {
            req.headers_mut().remove(http::header::CONTENT_LENGTH);
        }
    }
    let trailers = OwnTrailers::default();
    req.extensions_mut().insert(trailers.clone());
    let inner = std::mem::take(req.body_mut());
    let body = Decoded {
        inner,
        decoder: Decoder::new(declared, trailers),
        ready: VecDeque::new(),
        done: false,
    };
    *req.body_mut() = s3s::Body::from(Box::pin(body) as s3s::stream::DynByteStream);
}

/// Trailing headers decoded by [`accept_any`], kept in the request's extensions.
#[derive(Clone, Default)]
struct OwnTrailers(Arc<Mutex<Option<http::HeaderMap>>>);

/// The trailing headers of an aws-chunked body: s3s's when it verified the signature, ours when
/// signatures are not verified. Only complete once the body has been read to its end.
pub struct Trailers(Source);

enum Source {
    S3s(s3s::TrailingHeaders),
    Own(OwnTrailers),
}

impl Trailers {
    pub fn of(s3s: Option<s3s::TrailingHeaders>, extensions: &http::Extensions) -> Option<Self> {
        match s3s {
            Some(t) => Some(Self(Source::S3s(t))),
            None => extensions.get::<OwnTrailers>().cloned().map(|t| Self(Source::Own(t))),
        }
    }

    pub fn take(&self) -> Option<http::HeaderMap> {
        match &self.0 {
            Source::S3s(t) => t.take(),
            Source::Own(t) => t.0.lock().take(),
        }
    }
}

/// Longest chunk header line (`<hex size>;chunk-signature=<64 hex>`) accepted.
const MAX_META: usize = 1024;
/// Longest trailer block accepted.
const MAX_TRAILERS: usize = 16 * 1024;

enum State {
    /// Reading a chunk header line.
    Meta,
    /// Inside a chunk's data, with this many bytes left.
    Data(u64),
    /// Expecting the CRLF after a chunk's data.
    DataEnd,
    /// After the zero-length chunk: trailing headers, up to the end of the body.
    Trailers,
}

/// Incremental aws-chunked parser: `<hex size>[;chunk-signature=..]\r\n<data>\r\n ... 0\r\n` and
/// then optional `name:value\r\n` trailers. Signatures are skipped, not checked.
struct Decoder {
    state: State,
    buf: BytesMut,
    declared: Option<u64>,
    produced: u64,
    trailers: OwnTrailers,
}

fn malformed(what: &str) -> StdError {
    Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!("malformed aws-chunked body: {what}"),
    ))
}

impl Decoder {
    fn new(declared: Option<u64>, trailers: OwnTrailers) -> Self {
        Self {
            state: State::Meta,
            buf: BytesMut::new(),
            declared,
            produced: 0,
            trailers,
        }
    }

    /// Consumes `input`, queueing decoded data in `out`.
    fn feed(&mut self, mut input: Bytes, out: &mut VecDeque<Bytes>) -> Result<(), StdError> {
        while !input.is_empty() {
            match self.state {
                State::Meta => {
                    let line_end = input.iter().position(|&b| b == b'\n').map_or(input.len(), |i| i + 1);
                    self.buf.extend_from_slice(&input.split_to(line_end));
                    if self.buf.len() > MAX_META {
                        return Err(malformed("chunk header too long"));
                    }
                    if self.buf.ends_with(b"\n") {
                        let size = parse_meta(&self.buf)?;
                        self.buf.clear();
                        self.state = if size == 0 { State::Trailers } else { State::Data(size) };
                    }
                }
                State::Data(left) => {
                    let n = left.min(input.len() as u64);
                    out.push_back(input.split_to(n as usize));
                    self.produced += n;
                    self.state = if n == left { State::DataEnd } else { State::Data(left - n) };
                }
                State::DataEnd => {
                    let need = 2 - self.buf.len();
                    self.buf.extend_from_slice(&input.split_to(need.min(input.len())));
                    if self.buf.len() == 2 {
                        if &self.buf[..] != b"\r\n" {
                            return Err(malformed("missing CRLF after chunk data"));
                        }
                        self.buf.clear();
                        self.state = State::Meta;
                    }
                }
                State::Trailers => {
                    if self.buf.len() + input.len() > MAX_TRAILERS {
                        return Err(malformed("trailers too long"));
                    }
                    self.buf.extend_from_slice(&input.split_to(input.len()));
                }
            }
        }
        Ok(())
    }

    /// The body ended: publishes the trailers and checks the decoded length.
    fn finish(&mut self) -> Result<(), StdError> {
        if !matches!(self.state, State::Trailers) {
            return Err(malformed("ended before the final chunk"));
        }
        if self.declared.is_some_and(|n| n != self.produced) {
            return Err(malformed("length differs from x-amz-decoded-content-length"));
        }
        let mut map = http::HeaderMap::new();
        for line in self.buf[..].split(|&b| b == b'\n') {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            if line.is_empty() {
                continue;
            }
            let colon = line
                .iter()
                .position(|&b| b == b':')
                .ok_or_else(|| malformed("trailer without a colon"))?;
            let name = http::HeaderName::from_bytes(line[..colon].trim_ascii()).map_err(|_| malformed("trailer name"))?;
            if name == "x-amz-trailer-signature" {
                continue;
            }
            let value = http::HeaderValue::from_bytes(line[colon + 1..].trim_ascii()).map_err(|_| malformed("trailer value"))?;
            map.append(name, value);
        }
        if !map.is_empty() {
            *self.trailers.0.lock() = Some(map);
        }
        Ok(())
    }
}

/// The size on a chunk header line, ignoring any `;extension` such as the chunk signature.
fn parse_meta(line: &[u8]) -> Result<u64, StdError> {
    let line = line.strip_suffix(b"\r\n").ok_or_else(|| malformed("chunk header without CRLF"))?;
    let size = line.split(|&b| b == b';').next().unwrap_or(&[]);
    std::str::from_utf8(size)
        .ok()
        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .and_then(|s| u64::from_str_radix(s, 16).ok())
        .ok_or_else(|| malformed("chunk size"))
}

/// A request body with its aws-chunked framing removed.
struct Decoded {
    inner: s3s::Body,
    decoder: Decoder,
    ready: VecDeque<Bytes>,
    done: bool,
}

impl Stream for Decoded {
    type Item = Result<Bytes, StdError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = &mut *self;
        loop {
            if let Some(data) = this.ready.pop_front() {
                return Poll::Ready(Some(Ok(data)));
            }
            if this.done {
                return Poll::Ready(None);
            }
            let step = match Pin::new(&mut this.inner).poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Some(Ok(bytes))) => this.decoder.feed(bytes, &mut this.ready),
                Poll::Ready(Some(Err(e))) => Err(e),
                Poll::Ready(None) => {
                    this.done = true;
                    this.decoder.finish()
                }
            };
            if let Err(e) = step {
                this.done = true;
                this.ready.clear();
                return Poll::Ready(Some(Err(e)));
            }
        }
    }
}

impl ByteStream for Decoded {
    fn remaining_length(&self) -> RemainingLength {
        let ready: u64 = self.ready.iter().map(|b| b.len() as u64).sum();
        match self.decoder.declared {
            Some(n) => RemainingLength::new_exact(n.saturating_sub(self.decoder.produced - ready) as usize),
            None => RemainingLength::unknown(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(frames: &[&[u8]], declared: Option<u64>) -> (Result<Vec<u8>, String>, Option<http::HeaderMap>) {
        let trailers = OwnTrailers::default();
        let mut d = Decoder::new(declared, trailers.clone());
        let mut out = VecDeque::new();
        let mut run = || -> Result<Vec<u8>, StdError> {
            for f in frames {
                d.feed(Bytes::copy_from_slice(f), &mut out)?;
            }
            d.finish()?;
            Ok(out.iter().flat_map(|b| b.iter().copied()).collect())
        };
        let r = run().map_err(|e| e.to_string());
        let t = trailers.0.lock().take();
        (r, t)
    }

    const SIG: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn signed_chunks() {
        let body = format!("5;chunk-signature={SIG}\r\nhello\r\n6;chunk-signature={SIG}\r\n world\r\n0;chunk-signature={SIG}\r\n\r\n");
        let (r, t) = decode(&[body.as_bytes()], Some(11));
        assert_eq!(r.unwrap(), b"hello world");
        assert!(t.is_none());
    }

    #[test]
    fn unsigned_chunks_with_trailer() {
        let body = b"A\r\n0123456789\r\n0\r\nx-amz-checksum-crc32:AAAAAA==\r\n\r\n";
        let (r, t) = decode(&[body], Some(10));
        assert_eq!(r.unwrap(), b"0123456789");
        assert_eq!(t.unwrap()["x-amz-checksum-crc32"], "AAAAAA==");
    }

    #[test]
    fn signed_trailer_drops_its_signature() {
        let body = format!(
            "3;chunk-signature={SIG}\r\nabc\r\n0;chunk-signature={SIG}\r\nx-amz-checksum-sha1:x=\r\nx-amz-trailer-signature:{SIG}\r\n"
        );
        let (r, t) = decode(&[body.as_bytes()], None);
        assert_eq!(r.unwrap(), b"abc");
        let t = t.unwrap();
        assert_eq!(t.len(), 1);
        assert_eq!(t["x-amz-checksum-sha1"], "x=");
    }

    #[test]
    fn split_at_every_byte() {
        let body = format!("5;chunk-signature={SIG}\r\nhello\r\n0\r\nx-amz-checksum-crc32:AAAAAA==\r\n\r\n");
        let frames: Vec<&[u8]> = body.as_bytes().chunks(1).collect();
        let (r, t) = decode(&frames, Some(5));
        assert_eq!(r.unwrap(), b"hello");
        assert!(t.is_some());
    }

    #[test]
    fn rejects_bad_framing() {
        for body in [&b"zz\r\nhello\r\n0\r\n\r\n"[..], b"5\r\nhelloXX0\r\n\r\n", b"5\r\nhel", b"5\n"] {
            assert!(decode(&[body], None).0.is_err(), "{}", String::from_utf8_lossy(body));
        }
        // Declared length disagrees with the data.
        assert!(decode(&[b"5\r\nhello\r\n0\r\n\r\n"], Some(6)).0.is_err());
        // Truncated before the final chunk.
        assert!(decode(&[b"5\r\nhello\r\n"], Some(5)).0.is_err());
    }

    #[test]
    fn header_keys() {
        let v4 = "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/us-east-1/s3/aws4_request, SignedHeaders=host, Signature=ab";
        assert_eq!(header_access_key(v4).as_deref(), Some("AKIDEXAMPLE"));
        assert_eq!(header_access_key("AWS AKIDV2:c2ln").as_deref(), Some("AKIDV2"));
        assert_eq!(header_access_key("Bearer x"), None);
    }

    fn signed(uri: &str, auth: Option<&str>) -> (Option<Signed>, String, bool) {
        let mut req = http::Request::get(uri).body(s3s::Body::empty()).unwrap();
        if let Some(a) = auth {
            req.headers_mut().insert(http::header::AUTHORIZATION, a.parse().unwrap());
        }
        let marker = accept_any(&mut req, &Users::default()).cloned();
        let left = req.uri().to_string();
        (marker, left, req.headers().contains_key(http::header::AUTHORIZATION))
    }

    #[test]
    fn signed_requests_are_marked_and_unsigned_ones_are_not() {
        let (marker, uri, header) = signed(
            "/b/k",
            Some("AWS4-HMAC-SHA256 Credential=AK/20240101/r/s3/aws4_request, Signature=ab"),
        );
        let want = Signed {
            access_key: Some("AK".into()),
            presigned: false,
            user: UserId::DEFAULT,
        };
        assert_eq!((marker, uri.as_str(), header), (Some(want), "/b/k", false));

        let (marker, uri, _) = signed("/b/k?AWSAccessKeyId=AK2&Expires=9&Signature=x&versionId=1", None);
        let want = Signed {
            access_key: Some("AK2".into()),
            presigned: true,
            user: UserId::DEFAULT,
        };
        assert_eq!((marker, uri.as_str()), (Some(want), "/b/k?versionId=1"));

        // An Authorization header in a scheme we cannot read is still a signature.
        let (marker, ..) = signed("/b/k", Some("Bearer x"));
        assert_eq!(marker.map(|s| s.access_key), Some(None));

        let (marker, uri, _) = signed("/b/k?prefix=a", None);
        assert_eq!((marker, uri.as_str()), (None, "/b/k?prefix=a"));
    }

    #[test]
    fn the_access_key_picks_the_identity() {
        let users = Users::new("fakes3", "s", &["alt:ALTKEY:x".parse().unwrap()]).unwrap();
        let user_of = |uri: &str, auth: Option<&str>| {
            let mut req = http::Request::get(uri).body(s3s::Body::empty()).unwrap();
            if let Some(a) = auth {
                req.headers_mut().insert(http::header::AUTHORIZATION, a.parse().unwrap());
            }
            accept_any(&mut req, &users).map(|s| s.user)
        };
        let header = |key: &str| format!("AWS4-HMAC-SHA256 Credential={key}/20240101/r/s3/aws4_request, Signature=ab");
        assert_eq!(user_of("/b/k", Some(&header("ALTKEY"))), Some(UserId::DEFAULT.next()));
        assert_eq!(user_of("/b/k", Some(&header("fakes3"))), Some(UserId::DEFAULT));
        // Nobody's key is the default account; no signature at all is anonymous.
        assert_eq!(user_of("/b/k", Some(&header("stranger"))), Some(UserId::DEFAULT));
        assert_eq!(user_of("/b/k", None), None);
        let query = "/b/k?X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential=ALTKEY%2F20240101%2Fr%2Fs3%2Faws4_request&X-Amz-Signature=ab";
        assert_eq!(user_of(query, None), Some(UserId::DEFAULT.next()));
    }

    #[test]
    fn presigned_queries() {
        let v4 = "response-content-type=text%2Fplain&X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential=AKID%2F20240101%2Fus-east-1%2Fs3%2Faws4_request&X-Amz-Date=20240101T000000Z&X-Amz-Expires=60&X-Amz-SignedHeaders=host&X-Amz-Signature=ab";
        let (rest, key) = strip_presigned(v4).unwrap();
        assert_eq!(rest, "response-content-type=text%2Fplain");
        assert_eq!(key.as_deref(), Some("AKID"));
        let (rest, key) = strip_presigned("versionId=1&AWSAccessKeyId=AKIDV2&Expires=2000&Signature=c2ln%3D").unwrap();
        assert_eq!(rest, "versionId=1");
        assert_eq!(key.as_deref(), Some("AKIDV2"));
        assert!(strip_presigned("prefix=a&Expires=5").is_none());
        assert!(strip_presigned("uploads").is_none());
    }
}
