//! Presigned-URL expiry against the server clock.
//!
//! s3s checks `X-Amz-Expires` / `Expires` against the host clock, which a virtual clock does not
//! move. When the clock is shifted, this check runs first and rejects URLs the virtual clock says
//! have expired, so tests can expire a URL by advancing the clock. It cannot make s3s accept a URL
//! the host clock calls expired or not yet valid; see docs/control-plane.md.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Whether `query` belongs to a presigned URL that has expired at `now`.
pub fn is_expired(query: &str, now: SystemTime) -> bool {
    let mut date = None;
    let mut expires = None;
    let mut v2_expires = None;
    for (k, v) in form_urlencoded::parse(query.as_bytes()) {
        match &*k {
            "X-Amz-Date" => date = Some(v),
            "X-Amz-Expires" => expires = v.parse::<u64>().ok(),
            "Expires" => v2_expires = v.parse::<u64>().ok(),
            _ => {}
        }
    }
    if let (Some(date), Some(secs)) = (date, expires) {
        // Compact ISO 8601: 20300102T030405Z.
        let Some(start) = parse_amz_date(&date) else { return false };
        return now > start + Duration::from_secs(secs);
    }
    v2_expires.is_some_and(|secs| now > UNIX_EPOCH + Duration::from_secs(secs))
}

pub(crate) fn parse_amz_date(s: &str) -> Option<SystemTime> {
    if s.len() != 16 || !s.is_ascii() {
        return None;
    }
    let iso = format!(
        "{}-{}-{}T{}:{}:{}Z",
        &s[0..4],
        &s[4..6],
        &s[6..8],
        &s[9..11],
        &s[11..13],
        &s[13..15]
    );
    crate::control::parse_rfc3339(&iso)
}

/// The response S3 sends for an expired URL.
pub fn expired_response() -> s3s::HttpResponse {
    const BODY: &str =
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Error><Code>AccessDenied</Code><Message>Request has expired</Message></Error>";
    http::Response::builder()
        .status(http::StatusCode::FORBIDDEN)
        .header(http::header::CONTENT_TYPE, "application/xml")
        .header(http::header::CONTENT_LENGTH, BODY.len())
        .body(s3s::Body::from(BODY.to_owned()))
        .expect("static response parts are valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn v4_expiry() {
        // 2030-01-02T03:04:05Z = 1893553445
        let q = "X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Date=20300102T030405Z&X-Amz-Expires=3600&X-Amz-Signature=ab";
        assert!(!is_expired(q, at(1893553445 + 3600)));
        assert!(is_expired(q, at(1893553445 + 3601)));
    }

    #[test]
    fn v2_expiry() {
        assert!(!is_expired("Expires=2000&Signature=x", at(2000)));
        assert!(is_expired("Expires=2000&Signature=x", at(2001)));
    }

    #[test]
    fn not_presigned() {
        assert!(!is_expired("prefix=x&list-type=2", at(u32::MAX as u64)));
        assert!(!is_expired("", at(1)));
    }
}
