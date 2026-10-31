//! Virtual-host-style addressing: `<bucket>.<domain>` selects the bucket.
//!
//! Compared to the built-in s3s host parsers, a host that matches none of the configured domains
//! is always path-style (never a CNAME-style bucket, never an error), so clients reaching the
//! server by an arbitrary name (a Docker service name, `host.docker.internal`, an IP) keep working.

use s3s::S3Result;
use s3s::host::{S3Host, VirtualHost};

#[derive(Clone, Debug)]
pub struct Domains(Vec<String>);

fn strip_port(host: &str) -> &str {
    match host.rsplit_once(':') {
        Some((h, port)) if !h.ends_with(']') && !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => h,
        _ => host,
    }
}

impl Domains {
    pub fn new(domains: impl IntoIterator<Item = impl AsRef<str>>) -> Self {
        Self(
            domains
                .into_iter()
                .map(|d| d.as_ref().trim_matches('.').to_ascii_lowercase())
                .collect(),
        )
    }

    /// Splits `host` (with or without port) into the matching domain and the bucket in front of it.
    fn split<'a>(&'a self, host: &'a str) -> Option<(&'a str, Option<&'a str>)> {
        let host = strip_port(host).trim_end_matches('.');
        self.0.iter().find_map(|d| {
            let d = strip_port(d);
            if host.eq_ignore_ascii_case(d) {
                return Some((d, None));
            }
            let cut = host.len().checked_sub(d.len() + 1)?;
            let (bucket, suffix) = host.split_at(cut);
            let suffix = suffix.strip_prefix('.')?;
            (suffix.eq_ignore_ascii_case(d) && !bucket.is_empty()).then_some((d, Some(bucket)))
        })
    }

    /// The bucket a request addresses: from the host in virtual-host style, else the first path segment.
    pub fn bucket_of<'a>(&'a self, host: Option<&'a str>, path: &'a str) -> Option<&'a str> {
        if let Some(h) = host
            && let Some((_, Some(bucket))) = self.split(h)
        {
            return Some(bucket);
        }
        path.trim_start_matches('/').split('/').next().filter(|b| !b.is_empty())
    }
}

impl S3Host for Domains {
    fn parse_host_header<'a>(&'a self, host: &'a str) -> S3Result<VirtualHost<'a>> {
        Ok(match self.split(host) {
            Some((domain, Some(bucket))) => VirtualHost::new(domain).with_bucket(bucket),
            Some((domain, None)) => VirtualHost::new(domain),
            None => VirtualHost::new(host),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitting() {
        let d = Domains::new(["localhost", "s3.test:9000"]);
        let bucket = |h: &str| d.parse_host_header(h).unwrap().bucket().map(str::to_owned);
        assert_eq!(bucket("b.localhost:9000").as_deref(), Some("b"));
        assert_eq!(bucket("My.Bucket.LOCALHOST").as_deref(), Some("My.Bucket"));
        assert_eq!(bucket("localhost:9000"), None);
        assert_eq!(bucket("b.s3.test"), Some("b".into()));
        assert_eq!(bucket("fakes3:9000"), None);
        assert_eq!(bucket("my_service:9000"), None);
        assert_eq!(bucket("notlocalhost"), None);
        assert_eq!(bucket(".localhost"), None);
    }

    #[test]
    fn bucket_of_prefers_host() {
        let d = Domains::new(["localhost"]);
        assert_eq!(d.bucket_of(Some("b.localhost:1"), "/key"), Some("b"));
        assert_eq!(d.bucket_of(Some("localhost:1"), "/b/key"), Some("b"));
        assert_eq!(d.bucket_of(Some("127.0.0.1:1"), "/b"), Some("b"));
        assert_eq!(d.bucket_of(None, "/"), None);
    }
}
