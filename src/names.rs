//! Header names the per-request code looks up.
//!
//! `HeaderMap` hashes a custom header name (every `x-amz-*`) byte by byte with FNV, a dependent
//! multiply per byte: looking up `x-amz-decoded-content-length` costs about as much as parsing a
//! small request line. With a `&str` key it also case-folds and validates the name first. So the
//! per-request checks "is this header present" share one pass over the request's few headers
//! ([`scan`]) instead of doing a hashed lookup each, and the names that are still looked up or set
//! are `HeaderName::from_static` constants.

use http::{HeaderMap, HeaderName};

pub const X_AMZ_REQUEST_ID: HeaderName = HeaderName::from_static("x-amz-request-id");
pub const X_AMZ_ID_2: HeaderName = HeaderName::from_static("x-amz-id-2");
pub const X_AMZ_CONTENT_SHA256: HeaderName = HeaderName::from_static("x-amz-content-sha256");
pub const X_AMZ_DECODED_CONTENT_LENGTH: HeaderName = HeaderName::from_static("x-amz-decoded-content-length");
pub const X_AMZ_EXPECTED_BUCKET_OWNER: HeaderName = HeaderName::from_static("x-amz-expected-bucket-owner");
pub const X_AMZ_SOURCE_EXPECTED_BUCKET_OWNER: HeaderName = HeaderName::from_static("x-amz-source-expected-bucket-owner");

/// Which of the headers that change what an ordinary request does a request carries.
#[derive(Clone, Copy, Debug, Default)]
pub struct Present {
    pub content_md5: bool,
    pub copy_source: bool,
    pub expected_owner: bool,
    pub decoded_length: bool,
    pub source_expected_owner: bool,
}

/// One pass over `h`, switching on the name's length before comparing it.
pub fn scan(h: &HeaderMap) -> Present {
    let mut p = Present::default();
    for name in h.keys() {
        let n = name.as_str();
        match n.len() {
            11 => p.content_md5 |= n == "content-md5",
            17 => p.copy_source |= n == "x-amz-copy-source",
            27 => p.expected_owner |= n == "x-amz-expected-bucket-owner",
            28 => p.decoded_length |= n == "x-amz-decoded-content-length",
            34 => p.source_expected_owner |= n == "x-amz-source-expected-bucket-owner",
            _ => {}
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_finds_exactly_the_named_headers() {
        let mut h = HeaderMap::new();
        for (k, v) in [
            ("host", "h"),
            ("x-amz-date", "d"),
            ("content-md5", ""),
            ("x-amz-copy-source", "b/k"),
            ("x-amz-expected-bucket-owner", "1"),
            ("x-amz-decoded-content-length", "3"),
            ("x-amz-source-expected-bucket-owner", "1"),
        ] {
            h.append(HeaderName::from_static(k), v.parse().unwrap());
        }
        let p = scan(&h);
        assert!(p.content_md5 && p.copy_source && p.expected_owner && p.decoded_length && p.source_expected_owner);
        let p = scan(&HeaderMap::new());
        assert!(!(p.content_md5 || p.copy_source || p.expected_owner || p.decoded_length || p.source_expected_owner));
        // Near misses of the right length.
        let mut h = HeaderMap::new();
        h.insert(HeaderName::from_static("x-amz-copy-sourcf"), "x".parse().unwrap());
        h.insert(HeaderName::from_static("content-md6"), "x".parse().unwrap());
        let p = scan(&h);
        assert!(!p.copy_source && !p.content_md5);
    }
}
