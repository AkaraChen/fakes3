//! SigV2 requests under `--auth`: two things s3s gets wrong. Both only touch requests that are
//! SigV2-signed (an `Authorization: AWS ...` header, or `Signature=` in the query); SigV4 requests
//! pay for one header lookup. Without `--auth` signatures are not looked at, so none of this runs.
//! See docs/s3s-workarounds.md.
//!
//! * Dates. s3s answers a missing or unparseable `Date` / `x-amz-date` with 400 `InvalidRequest`
//!   and parses `x-amz-date` as SigV4's compact ISO 8601 only. S3 answers 403 `AccessDenied`, or
//!   403 `RequestTimeTooSkewed` for a time that parses but is more than 15 minutes off. [`bad_date`]
//!   judges the headers before s3s does and produces S3's answer. A well-formed RFC 1123
//!   `x-amz-date` (which S3 accepts) still reaches s3s and is refused there.
//! * The canonical resource of a bucket-level request. botocore signs `/bucket/` for a request line
//!   of `/bucket`, s3s signs the request path as it is, so botocore's bucket-level calls never
//!   verify. S3 accepts both. [`call_retrying`] runs such a request as it is first and, when s3s
//!   answers `SignatureDoesNotMatch`, once more with the slash appended. The first answer comes
//!   before the operation runs, so repeating is safe.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use http_body_util::BodyExt;
use hyper::body::Body as _;
use s3s::dto::{Timestamp, TimestampFormat};
use s3s::{S3Error, S3ErrorCode};

/// How a request is SigV2-signed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Header,
    Presigned,
}

pub fn kind<B>(req: &http::Request<B>) -> Option<Kind> {
    match req.headers().get(http::header::AUTHORIZATION) {
        Some(auth) => auth.as_bytes().starts_with(b"AWS ").then_some(Kind::Header),
        None => req
            .uri()
            .query()
            .filter(|q| q.split('&').any(|p| p.starts_with("Signature=")))
            .map(|_| Kind::Presigned),
    }
}

/// Largest bucket-level body that is kept to be replayed. Bigger ones (and unsized ones) are not
/// retried and see s3s's answer.
const MAX_REPLAY: u64 = 4 << 20;
/// How far a request's time may be from the server's, as in S3 and s3s.
const MAX_SKEW: Duration = Duration::from_secs(15 * 60);

fn refuse(code: S3ErrorCode, message: &'static str) -> s3s::HttpResponse {
    S3Error::with_message(code, message).to_http_response().unwrap_or_else(|_| {
        let mut r = s3s::HttpResponse::new(s3s::Body::empty());
        *r.status_mut() = http::StatusCode::FORBIDDEN;
        r
    })
}

/// S3's refusal of a header-signed request whose `x-amz-date` (or else `Date`) is missing,
/// unparseable, before 1970 or too far from `now`.
pub fn bad_date(headers: &http::HeaderMap, now: SystemTime) -> Option<s3s::HttpResponse> {
    let invalid = || {
        refuse(
            S3ErrorCode::AccessDenied,
            "AWS authentication requires a valid Date or x-amz-date header",
        )
    };
    let Some(value) = headers.get("x-amz-date").or_else(|| headers.get(http::header::DATE)) else {
        return Some(invalid());
    };
    let Some(time) = value.to_str().ok().and_then(parse_date).filter(|t| *t >= UNIX_EPOCH) else {
        return Some(invalid());
    };
    let skew = now.duration_since(time).unwrap_or_else(|e| e.duration());
    (skew > MAX_SKEW).then(|| {
        refuse(
            S3ErrorCode::RequestTimeTooSkewed,
            "The difference between the request time and the current time is too large.",
        )
    })
}

fn parse_date(s: &str) -> Option<SystemTime> {
    match Timestamp::parse(TimestampFormat::HttpDate, s) {
        Ok(t) => Some(time::OffsetDateTime::from(t).into()),
        Err(_) => crate::presign::parse_amz_date(s),
    }
}

/// A request for `/bucket`, which botocore may have signed as `/bucket/`.
pub fn is_bucket_level<B>(req: &http::Request<B>) -> bool {
    let path = req.uri().path();
    path.len() > 1 && !path[1..].contains('/')
}

/// Calls `s3`, and repeats the call for `/bucket/` if the signature did not match for `/bucket`.
pub async fn call_retrying(s3: &s3s::service::S3Service, req: http::Request<s3s::Body>) -> Result<s3s::HttpResponse, s3s::HttpError> {
    let (parts, body) = req.into_parts();
    let len = parts
        .headers
        .get(http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if !len.map_or(body.is_end_stream(), |n| n <= MAX_REPLAY) {
        return s3.call(http::Request::from_parts(parts, body)).await;
    }
    let body = body.collect().await.map_err(s3s::HttpError::from_std_error)?.to_bytes();
    let again = slashed(&parts, body.clone());
    let head = parts.method == http::Method::HEAD;
    let resp = s3.call(http::Request::from_parts(parts, s3s::Body::from(body))).await?;
    if resp.status() != http::StatusCode::FORBIDDEN {
        return Ok(resp);
    }
    // A HEAD response has no body to say why. HEAD changes nothing, so it can be asked twice; the
    // second answer counts only if it is not another refusal.
    let mismatch = head
        || resp
            .body()
            .bytes()
            .is_some_and(|b| crate::reqid::contains(&b, b"<Code>SignatureDoesNotMatch</Code>"));
    let Some(again) = again.filter(|_| mismatch) else { return Ok(resp) };
    match s3.call(again).await {
        Ok(second) if !head || second.status() != http::StatusCode::FORBIDDEN => Ok(second),
        _ => Ok(resp),
    }
}

/// The same request for `<path>/`.
fn slashed(parts: &http::request::Parts, body: Bytes) -> Option<http::Request<s3s::Body>> {
    let pq = parts.uri.path_and_query()?;
    let target = match pq.query() {
        Some(q) => format!("{}/?{q}", pq.path()),
        None => format!("{}/", pq.path()),
    };
    let mut uri = parts.uri.clone().into_parts();
    uri.path_and_query = Some(target.parse().ok()?);
    let mut req = http::Request::new(s3s::Body::from(body));
    *req.method_mut() = parts.method.clone();
    *req.uri_mut() = http::Uri::from_parts(uri).ok()?;
    *req.version_mut() = parts.version;
    *req.headers_mut() = parts.headers.clone();
    *req.extensions_mut() = parts.extensions.clone();
    Some(req)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    fn headers(pairs: &[(&str, &str)]) -> http::HeaderMap {
        let mut h = http::HeaderMap::new();
        for (k, v) in pairs {
            h.insert(http::HeaderName::from_bytes(k.as_bytes()).unwrap(), v.parse().unwrap());
        }
        h
    }

    fn verdict(pairs: &[(&str, &str)]) -> Option<(u16, String)> {
        // 2030-01-02T03:04:05Z
        bad_date(&headers(pairs), at(1893553445)).map(|r| {
            let body = String::from_utf8(r.body().bytes().unwrap().to_vec()).unwrap();
            let code = body.split("<Code>").nth(1).unwrap().split("</Code>").next().unwrap().to_owned();
            (r.status().as_u16(), code)
        })
    }

    #[test]
    fn dates() {
        let denied = Some((403, "AccessDenied".to_owned()));
        let skewed = Some((403, "RequestTimeTooSkewed".to_owned()));
        assert_eq!(verdict(&[("date", "Wed, 02 Jan 2030 03:04:05 GMT")]), None);
        assert_eq!(verdict(&[("date", "Wed, 02 Jan 2030 03:10:00 GMT")]), None);
        assert_eq!(verdict(&[("x-amz-date", "Wed, 02 Jan 2030 03:04:05 GMT")]), None);
        assert_eq!(verdict(&[("x-amz-date", "20300102T030405Z")]), None);
        // x-amz-date wins over Date.
        assert_eq!(
            verdict(&[("x-amz-date", "Bad Date"), ("date", "Wed, 02 Jan 2030 03:04:05 GMT")]),
            denied
        );
        assert_eq!(verdict(&[]), denied);
        assert_eq!(verdict(&[("x-amz-date", "")]), denied);
        assert_eq!(verdict(&[("x-amz-date", "Bad Date")]), denied);
        assert_eq!(verdict(&[("x-amz-date", "Tue, 07 Jul 1950 21:53:04 GMT")]), denied);
        assert_eq!(verdict(&[("x-amz-date", "Tue, 07 Jul 2010 21:53:04 GMT")]), skewed);
        assert_eq!(verdict(&[("date", "Tue, 07 Jul 9999 21:53:04 GMT")]), skewed);
        assert_eq!(verdict(&[("date", "Wed, 02 Jan 2030 03:30:00 GMT")]), skewed);
    }

    #[test]
    fn kinds() {
        let req = |auth: Option<&str>, uri: &str| {
            let mut b = http::Request::builder().uri(uri);
            if let Some(a) = auth {
                b = b.header("authorization", a);
            }
            b.body(()).unwrap()
        };
        assert_eq!(kind(&req(Some("AWS fakes3:c2ln"), "/bkt")), Some(Kind::Header));
        assert_eq!(kind(&req(Some("AWS4-HMAC-SHA256 Credential=x"), "/bkt")), None);
        assert_eq!(
            kind(&req(None, "/bkt?AWSAccessKeyId=a&Expires=1&Signature=x")),
            Some(Kind::Presigned)
        );
        assert_eq!(kind(&req(None, "/bkt?X-Amz-Signature=x")), None);
        assert_eq!(kind(&req(None, "/bkt?list-type=2")), None);
        assert_eq!(kind(&req(None, "/bkt")), None);
    }

    #[test]
    fn bucket_level_paths() {
        let is = |uri: &str| is_bucket_level(&http::Request::builder().uri(uri).body(()).unwrap());
        assert!(is("/bkt"));
        assert!(is("/bkt?acl"));
        assert!(!is("/"));
        assert!(!is("/bkt/"));
        assert!(!is("/bkt/key"));
    }
}
