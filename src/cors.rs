//! CORS: configuration validation, rule matching and the response headers S3 adds.
//!
//! s3s does not route `OPTIONS`, so preflights are answered by the service wrapper in `lib.rs`,
//! which also adds the headers to actual responses. Both only run for requests that carry an
//! `Origin` header (or are `OPTIONS`), so buckets and clients that do not use CORS pay nothing.

use http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use s3s::dto::{CORSConfiguration, CORSRule};
use s3s::{S3Result, s3_error};

use crate::host::Domains;
use crate::policy::wildcard;
use crate::store::Store;

const METHODS: [&str; 5] = ["GET", "PUT", "POST", "DELETE", "HEAD"];

pub fn validate(cfg: &CORSConfiguration) -> S3Result<()> {
    if cfg.cors_rules.is_empty() || cfg.cors_rules.len() > 100 {
        return Err(s3_error!(
            MalformedXML,
            "The XML you provided was not well-formed or did not validate against our published schema"
        ));
    }
    for r in &cfg.cors_rules {
        if r.allowed_origins.is_empty() || r.allowed_methods.is_empty() {
            return Err(s3_error!(
                MalformedXML,
                "The XML you provided was not well-formed or did not validate against our published schema"
            ));
        }
        if let Some(m) = r.allowed_methods.iter().find(|m| !METHODS.contains(&m.as_str())) {
            return Err(s3_error!(
                InvalidRequest,
                "Found unsupported HTTP method in CORS config. Unsupported method is {m}"
            ));
        }
        if let Some(o) = r.allowed_origins.iter().find(|o| o.matches('*').count() > 1) {
            return Err(s3_error!(
                InvalidRequest,
                "AllowedOrigin \"{o}\" can not have more than one wildcard."
            ));
        }
        if let Some(h) = r.allowed_headers.iter().flatten().find(|h| h.matches('*').count() > 1) {
            return Err(s3_error!(
                InvalidRequest,
                "AllowedHeader \"{h}\" can not have more than one wildcard."
            ));
        }
    }
    Ok(())
}

/// The first rule that allows `origin` to use `method` with all of `request_headers`.
pub fn find<'a>(cfg: &'a CORSConfiguration, origin: &str, method: &str, request_headers: &[&str]) -> Option<&'a CORSRule> {
    cfg.cors_rules.iter().find(|r| {
        r.allowed_origins.iter().any(|o| wildcard(o, origin, false))
            && r.allowed_methods.iter().any(|m| m == method)
            && request_headers
                .iter()
                .all(|h| r.allowed_headers.iter().flatten().any(|a| wildcard(a, h, true)))
    })
}

fn put(h: &mut HeaderMap, name: &'static str, value: &str) {
    if let Ok(v) = HeaderValue::from_str(value) {
        h.insert(HeaderName::from_static(name), v);
    }
}

/// Adds the CORS headers for a matched rule. `preflight_headers` is the raw
/// `Access-Control-Request-Headers` value of a preflight.
pub fn apply(h: &mut HeaderMap, rule: &CORSRule, origin: &str, preflight_headers: Option<&str>) {
    let any_origin = rule.allowed_origins.iter().any(|o| o == "*");
    put(h, "access-control-allow-origin", if any_origin { "*" } else { origin });
    let vary = match preflight_headers {
        Some(_) => "Origin, Access-Control-Request-Headers, Access-Control-Request-Method",
        None => "Origin",
    };
    h.append(http::header::VARY, HeaderValue::from_static(vary));
    put(h, "access-control-allow-methods", &rule.allowed_methods.join(", "));
    if let Some(req) = preflight_headers
        && !req.is_empty()
    {
        put(h, "access-control-allow-headers", req);
    }
    if let Some(e) = rule.expose_headers.as_ref().filter(|e| !e.is_empty()) {
        put(h, "access-control-expose-headers", &e.join(", "));
    }
    if let Some(age) = rule.max_age_seconds {
        put(h, "access-control-max-age", &age.to_string());
    }
    if !any_origin {
        put(h, "access-control-allow-credentials", "true");
    }
}

fn error_response(status: StatusCode, code: &str, message: &str, method: &str) -> http::Response<s3s::Body> {
    let body = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Error><Code>{code}</Code><Message>{message}</Message>\
         <Method>{method}</Method><ResourceType>BUCKET</ResourceType></Error>"
    );
    let mut resp = http::Response::new(s3s::Body::from(bytes::Bytes::from(body)));
    *resp.status_mut() = status;
    resp.headers_mut()
        .insert(http::header::CONTENT_TYPE, HeaderValue::from_static("application/xml"));
    resp
}

fn forbidden(method: &str, message: &str) -> http::Response<s3s::Body> {
    error_response(
        StatusCode::FORBIDDEN,
        "AccessForbidden",
        &format!("CORSResponse: {message}"),
        method,
    )
}

fn header<'a>(req: &'a http::Request<hyper::body::Incoming>, name: &str) -> Option<&'a str> {
    req.headers().get(name).and_then(|v| v.to_str().ok())
}

/// Answers a preflight `OPTIONS` request from the bucket's CORS configuration.
pub fn preflight(store: &Store, domains: &Domains, req: &http::Request<hyper::body::Incoming>) -> http::Response<s3s::Body> {
    let Some(bucket) = domains.bucket_of(header(req, "host"), req.uri().path()) else {
        return forbidden("OPTIONS", NOT_ENABLED);
    };
    let Some(bucket) = store.bucket(bucket) else {
        return error_response(
            StatusCode::NOT_FOUND,
            "NoSuchBucket",
            "The specified bucket does not exist",
            "OPTIONS",
        );
    };
    let (Some(origin), Some(method)) = (header(req, "origin"), header(req, "access-control-request-method")) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "BadRequest",
            "Insufficient information. Origin request header needed.",
            "OPTIONS",
        );
    };
    let Some(cfg) = bucket.config.cors.read().clone() else {
        return forbidden(method, NOT_ENABLED);
    };
    let requested = header(req, "access-control-request-headers").unwrap_or("");
    let names: Vec<&str> = requested.split(',').map(str::trim).filter(|h| !h.is_empty()).collect();
    let Some(rule) = find(&cfg, origin, method, &names) else {
        return forbidden(method, NOT_ALLOWED);
    };
    let mut resp = http::Response::new(s3s::Body::empty());
    apply(resp.headers_mut(), rule, origin, Some(requested));
    resp
}

/// What a response needs to know about its request to get CORS headers.
pub struct RequestInfo {
    origin: String,
    method: http::Method,
    bucket: Option<String>,
}

impl RequestInfo {
    pub fn of(req: &http::Request<hyper::body::Incoming>, domains: &Domains) -> Self {
        Self {
            origin: header(req, "origin").unwrap_or_default().to_owned(),
            method: req.method().clone(),
            bucket: domains.bucket_of(header(req, "host"), req.uri().path()).map(str::to_owned),
        }
    }

    /// Adds CORS headers to the response of an actual (non-preflight) request, when a rule allows it.
    pub fn decorate(self, store: &Store, resp: &mut http::Response<s3s::Body>) {
        let Some(bucket) = self.bucket.and_then(|b| store.bucket(&b)) else {
            return;
        };
        let Some(cfg) = bucket.config.cors.read().clone() else { return };
        if let Some(rule) = find(&cfg, &self.origin, self.method.as_str(), &[]) {
            apply(resp.headers_mut(), rule, &self.origin, None);
        }
    }
}

pub const NOT_ENABLED: &str = "CORS is not enabled for this bucket.";
pub const NOT_ALLOWED: &str = "This CORS request is not allowed. This is usually because the evalution of Origin, request method / \
     Access-Control-Request-Method or Access-Control-Request-Headers are not whitelisted by the resource's CORS spec.";

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(origins: &[&str], methods: &[&str], headers: &[&str]) -> CORSRule {
        CORSRule {
            allowed_origins: origins.iter().map(|s| s.to_string()).collect(),
            allowed_methods: methods.iter().map(|s| s.to_string()).collect(),
            allowed_headers: (!headers.is_empty()).then(|| headers.iter().map(|s| s.to_string()).collect()),
            expose_headers: Some(vec!["ETag".into()]),
            max_age_seconds: Some(600),
            id: None,
        }
    }

    #[test]
    fn matching() {
        let cfg = CORSConfiguration {
            cors_rules: vec![
                rule(&["https://*.example.com"], &["GET"], &[]),
                rule(&["*"], &["PUT"], &["x-amz-*", "content-type"]),
            ],
        };
        assert!(find(&cfg, "https://app.example.com", "GET", &[]).is_some());
        assert!(find(&cfg, "https://evil.test", "GET", &[]).is_none());
        assert!(find(&cfg, "https://app.example.com", "PUT", &["content-type", "x-amz-meta-a"]).is_some());
        assert!(find(&cfg, "https://app.example.com", "PUT", &["authorization"]).is_none());
    }

    #[test]
    fn headers() {
        let r = rule(&["https://a.test"], &["GET", "PUT"], &["*"]);
        let mut h = HeaderMap::new();
        apply(&mut h, &r, "https://a.test", Some("content-type"));
        assert_eq!(h["access-control-allow-origin"], "https://a.test");
        assert_eq!(h["access-control-allow-methods"], "GET, PUT");
        assert_eq!(h["access-control-allow-headers"], "content-type");
        assert_eq!(h["access-control-max-age"], "600");
        assert_eq!(h["access-control-allow-credentials"], "true");
        let star = rule(&["*"], &["GET"], &[]);
        let mut h = HeaderMap::new();
        apply(&mut h, &star, "https://a.test", None);
        assert_eq!(h["access-control-allow-origin"], "*");
        assert!(!h.contains_key("access-control-allow-credentials"));
    }

    #[test]
    fn validation() {
        let ok = CORSConfiguration {
            cors_rules: vec![rule(&["*"], &["GET"], &[])],
        };
        assert!(validate(&ok).is_ok());
        let bad_method = CORSConfiguration {
            cors_rules: vec![rule(&["*"], &["PATCH"], &[])],
        };
        assert!(validate(&bad_method).is_err());
        let two_stars = CORSConfiguration {
            cors_rules: vec![rule(&["*.*"], &["GET"], &[])],
        };
        assert!(validate(&two_stars).is_err());
    }
}
