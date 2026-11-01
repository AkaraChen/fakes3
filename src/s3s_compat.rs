//! Two more places where s3s answers differently from S3. See docs/s3s-workarounds.md.
//!
//! * POST policy conditions. s3s refuses a form that breaks a policy condition with 400
//!   `InvalidPolicyDocument`; S3 answers 403 `AccessDenied: Invalid according to Policy`. A
//!   malformed policy stays 400 `InvalidPolicyDocument`, as in S3. [`policy_violation`] rewrites
//!   the error response of a signed form, which [`signed_form`] finds out.
//! * SigV4 with a `Date` header. S3 takes the request time from `x-amz-date` or else `Date` (what
//!   botocore sends when a `Date` is set); s3s insists on `x-amz-date` (`missing header:
//!   x-amz-date`). [`date_to_amz_date`] adds the `x-amz-date` that `Date` stands for. The signature
//!   covers the time through the string to sign, so a made-up value could not verify.
//! * A body without a length. s3s refuses a `Transfer-Encoding: chunked` upload that has no
//!   `Content-Length` (`MissingContentLength`) when it verifies the signature, S3 accepts it.
//!   [`with_length`] reads such a body, up to [`MAX_UNSIZED`], and gives s3s a `Content-Length`.

use bytes::Bytes;
use http_body_util::{BodyExt, Limited};

/// What s3s's POST policy checks start their messages with.
const POLICY_MESSAGES: [&[u8]; 3] = [b"<Message>Policy condition ", b"<Message>Bucket in form field", b"<Message>x-amz-"];

/// Turns s3s's refusal of a form that violates its policy into S3's.
pub fn policy_violation(resp: &mut s3s::HttpResponse) {
    use crate::reqid::contains;
    let Some(body) = resp.body().bytes() else { return };
    if !contains(&body, b"<Code>InvalidPolicyDocument</Code>") || !POLICY_MESSAGES.iter().any(|m| contains(&body, m)) {
        return;
    }
    let Ok(xml) = std::str::from_utf8(&body) else { return };
    let xml = xml
        .replacen("<Code>InvalidPolicyDocument</Code>", "<Code>AccessDenied</Code>", 1)
        .replacen("<Message>", "<Message>Invalid according to Policy: ", 1);
    *resp.status_mut() = http::StatusCode::FORBIDDEN;
    if resp.headers().contains_key(http::header::CONTENT_LENGTH) {
        resp.headers_mut().insert(http::header::CONTENT_LENGTH, xml.len().into());
    }
    *resp.body_mut() = s3s::Body::from(Bytes::from(xml));
}

/// For a SigV4 header-signed request that has a `Date` and no `x-amz-date`, adds the latter.
pub fn date_to_amz_date(headers: &mut http::HeaderMap) {
    const X_AMZ_DATE: http::HeaderName = http::HeaderName::from_static("x-amz-date");
    if headers.contains_key(X_AMZ_DATE) || !headers.contains_key(http::header::AUTHORIZATION) {
        return;
    }
    let Some(date) = headers.get(http::header::DATE).and_then(|v| v.to_str().ok()) else {
        return;
    };
    // botocore writes the zone as `-0000`, not `GMT`.
    let date = match date.strip_suffix("-0000").or_else(|| date.strip_suffix("+0000")) {
        Some(rest) => std::borrow::Cow::Owned(format!("{rest}GMT")),
        None => std::borrow::Cow::Borrowed(date),
    };
    let Ok(t) = s3s::dto::Timestamp::parse(s3s::dto::TimestampFormat::HttpDate, &date) else {
        return;
    };
    let t = time::OffsetDateTime::from(t);
    let iso = format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute(),
        t.second()
    );
    if let Ok(v) = http::HeaderValue::from_str(&iso) {
        headers.insert(X_AMZ_DATE, v);
    }
}

/// Largest body read to learn its length.
pub const MAX_UNSIZED: usize = 64 << 20;

/// Whether `headers` describe a chunked body that has no length of its own to give s3s.
pub fn is_unsized(headers: &http::HeaderMap) -> bool {
    headers.contains_key(http::header::TRANSFER_ENCODING)
        && !headers.contains_key(http::header::CONTENT_LENGTH)
        // aws-chunked bodies are decoded by length (`x-amz-decoded-content-length`) and stream.
        && !headers.contains_key(&crate::names::X_AMZ_DECODED_CONTENT_LENGTH)
}

/// Reads `req`'s body and sets `Content-Length` to its size. A body over [`MAX_UNSIZED`] is
/// refused as before: 411 `MissingContentLength`.
pub async fn with_length(req: http::Request<s3s::Body>) -> Result<http::Request<s3s::Body>, s3s::HttpResponse> {
    let (mut parts, body) = req.into_parts();
    let refuse = || {
        s3s::S3Error::with_message(s3s::S3ErrorCode::MissingContentLength, "missing header: content-length")
            .to_http_response()
            .unwrap_or_else(|_| s3s::HttpResponse::new(s3s::Body::empty()))
    };
    let body = Limited::new(body, MAX_UNSIZED).collect().await.map_err(|_| refuse())?.to_bytes();
    parts.headers.insert(http::header::CONTENT_LENGTH, body.len().into());
    Ok(http::Request::from_parts(parts, s3s::Body::from(body)))
}

/// Largest POST form read to look for its signature field.
const MAX_FORM: u64 = 8 << 20;

/// Whether `req`, a POST, is a `multipart/form-data` form of at most [`MAX_FORM`] bytes that has a
/// signature field. s3s treats a form without one as anonymous and still judges its policy, where
/// S3 refuses it for the missing signature (400), so only signed forms get [`policy_violation`].
/// Reads the form and puts it back.
pub async fn signed_form(req: &mut http::Request<s3s::Body>) -> bool {
    let headers = req.headers();
    let form = headers
        .get(http::header::CONTENT_TYPE)
        .is_some_and(|v| v.as_bytes().starts_with(b"multipart/form-data"));
    let small = headers
        .get(http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .is_some_and(|n| n <= MAX_FORM);
    if !(form && small) {
        return false;
    }
    let Ok(body) = std::mem::take(req.body_mut()).collect().await.map(|b| b.to_bytes()) else {
        return false;
    };
    // The fields come before the file part (some clients, like Python's requests, give every field a filename).
    let fields = body
        .windows(11)
        .position(|w| w == b"name=\"file\"")
        .map_or(&body[..], |end| &body[..end]);
    let signed = fields.windows(10).any(|w| w.eq_ignore_ascii_case(b"signature\""));
    *req.body_mut() = s3s::Body::from(body);
    signed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error(code: &str, message: &str) -> s3s::HttpResponse {
        let xml = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><Error><Code>{code}</Code><Message>{message}</Message></Error>");
        http::Response::builder()
            .status(400)
            .header(http::header::CONTENT_LENGTH, xml.len())
            .body(s3s::Body::from(xml))
            .unwrap()
    }

    fn text(resp: &s3s::HttpResponse) -> String {
        String::from_utf8(resp.body().bytes().unwrap().to_vec()).unwrap()
    }

    #[test]
    fn violated_conditions_become_access_denied() {
        for message in [
            "Policy condition &apos;eq&apos; for field &apos;bucket&apos; failed: expected &apos;a&apos;, got &apos;b&apos;",
            "Bucket in form field &apos;a&apos; does not match bucket in URL &apos;b&apos;",
            "x-amz-date does not match policy",
        ] {
            let mut resp = error("InvalidPolicyDocument", message);
            policy_violation(&mut resp);
            let body = text(&resp);
            assert_eq!(resp.status(), 403, "{message}");
            assert!(body.contains("<Code>AccessDenied</Code>"), "{body}");
            assert!(
                body.contains(&format!("<Message>Invalid according to Policy: {message}</Message>")),
                "{body}"
            );
            assert_eq!(resp.headers()[http::header::CONTENT_LENGTH], body.len().to_string().as_str());
        }
    }

    #[test]
    fn other_errors_are_left_alone() {
        let mut resp = error("InvalidPolicyDocument", "failed to parse POST policy");
        policy_violation(&mut resp);
        assert_eq!(resp.status(), 400);
        assert!(text(&resp).contains("<Code>InvalidPolicyDocument</Code>"));
        let mut resp = error("NoSuchBucket", "Policy condition &apos;eq&apos;");
        policy_violation(&mut resp);
        assert_eq!(resp.status(), 400);
    }

    #[test]
    fn date_becomes_amz_date() {
        let run = |pairs: &[(&str, &str)]| {
            let mut m = http::HeaderMap::new();
            for (k, v) in pairs {
                m.insert(http::HeaderName::from_bytes(k.as_bytes()).unwrap(), v.parse().unwrap());
            }
            date_to_amz_date(&mut m);
            m.get("x-amz-date").map(|v| v.to_str().unwrap().to_owned())
        };
        let date = ("date", "Wed, 02 Jan 2030 03:04:05 GMT");
        let auth = ("authorization", "AWS4-HMAC-SHA256 Credential=a/20300102/us-east-1/s3/aws4_request");
        assert_eq!(run(&[date, auth]).as_deref(), Some("20300102T030405Z"));
        let zoned = ("date", "Wed, 02 Jan 2030 03:04:05 -0000");
        assert_eq!(run(&[zoned, auth]).as_deref(), Some("20300102T030405Z"));
        assert_eq!(
            run(&[date, auth, ("x-amz-date", "20300102T030406Z")]).as_deref(),
            Some("20300102T030406Z")
        );
        assert_eq!(run(&[date]), None);
        assert_eq!(run(&[auth]), None);
        assert_eq!(run(&[("date", "garbage"), auth]), None);
    }

    #[test]
    fn unsized_bodies() {
        let h = |pairs: &[(&str, &str)]| {
            let mut m = http::HeaderMap::new();
            for (k, v) in pairs {
                m.insert(http::HeaderName::from_bytes(k.as_bytes()).unwrap(), v.parse().unwrap());
            }
            is_unsized(&m)
        };
        assert!(h(&[("transfer-encoding", "chunked")]));
        assert!(!h(&[("transfer-encoding", "chunked"), ("content-length", "3")]));
        assert!(!h(&[("transfer-encoding", "chunked"), ("x-amz-decoded-content-length", "3")]));
        assert!(!h(&[]));
    }
}
