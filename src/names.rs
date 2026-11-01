//! Header names the per-request code looks up.
//!
//! `HeaderMap::get("x-amz-...")` with a `&str` lower-cases and validates the name on every call
//! (`parse_hdr`), which costs more than the lookup itself; with `HeaderName::from_static` that work
//! happens at compile time. Every header read or written on the path of an ordinary request is
//! named here.

use http::HeaderName;

pub const X_AMZ_REQUEST_ID: HeaderName = HeaderName::from_static("x-amz-request-id");
pub const X_AMZ_ID_2: HeaderName = HeaderName::from_static("x-amz-id-2");
pub const X_AMZ_COPY_SOURCE: HeaderName = HeaderName::from_static("x-amz-copy-source");
pub const X_AMZ_DECODED_CONTENT_LENGTH: HeaderName = HeaderName::from_static("x-amz-decoded-content-length");
pub const X_AMZ_CONTENT_SHA256: HeaderName = HeaderName::from_static("x-amz-content-sha256");
pub const X_AMZ_EXPECTED_BUCKET_OWNER: HeaderName = HeaderName::from_static("x-amz-expected-bucket-owner");
pub const X_AMZ_SOURCE_EXPECTED_BUCKET_OWNER: HeaderName = HeaderName::from_static("x-amz-source-expected-bucket-owner");
pub const CONTENT_MD5: HeaderName = HeaderName::from_static("content-md5");
