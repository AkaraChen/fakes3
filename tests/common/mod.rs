//! A minimal HTTP/1.1 client with SigV4 signing, enough to drive the server over a real socket.

#![allow(dead_code)]

use std::net::SocketAddr;
use std::time::{SystemTime, UNIX_EPOCH};

use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub const ACCESS: &str = "fakes3";
pub const SECRET: &str = "fakes3secret";
const REGION: &str = "us-east-1";

/// A SigV4 `Authorization` header nobody computed. Unless `Config::auth` is on, it is enough to
/// make a hand-written request count as signed (and so act as the bucket owner).
pub const UNVERIFIED_AUTH: &str = "AWS4-HMAC-SHA256 Credential=anyone/20240101/us-east-1/s3/aws4_request, SignedHeaders=host, Signature=00";

pub struct Resp {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Resp {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The `<Code>` of an error response.
    pub fn code(&self) -> &str {
        self.body
            .split("<Code>")
            .nth(1)
            .and_then(|s| s.split("</Code>").next())
            .unwrap_or("")
    }
}

pub struct Req<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub query: &'a str,
    pub headers: Vec<(&'a str, &'a str)>,
    pub body: &'a [u8],
    pub signed: bool,
    pub host: Option<String>,
    /// Send the body with `Transfer-Encoding: chunked` and no `Content-Length`.
    pub chunked: bool,
    /// Sign with a `Date` header (RFC 1123) instead of `x-amz-date`, as botocore does when a
    /// `Date` is set.
    pub date_header: bool,
}

pub fn req<'a>(method: &'a str, path: &'a str) -> Req<'a> {
    Req {
        method,
        path,
        query: "",
        headers: Vec::new(),
        body: b"",
        signed: true,
        host: None,
        chunked: false,
        date_header: false,
    }
}

impl<'a> Req<'a> {
    pub fn query(mut self, q: &'a str) -> Self {
        self.query = q;
        self
    }
    pub fn header(mut self, k: &'a str, v: &'a str) -> Self {
        self.headers.push((k, v));
        self
    }
    pub fn body(mut self, b: &'a [u8]) -> Self {
        self.body = b;
        self
    }
    pub fn anon(mut self) -> Self {
        self.signed = false;
        self
    }
    /// Unsigned as far as the signature goes, but with an `Authorization` header: a request that
    /// carries a signature nobody can verify.
    pub fn unverified(self) -> Self {
        self.anon().header("authorization", UNVERIFIED_AUTH)
    }
    pub fn date_header(mut self) -> Self {
        self.date_header = true;
        self
    }
    pub fn chunked(mut self) -> Self {
        self.chunked = true;
        self
    }
    pub fn host(mut self, h: &str) -> Self {
        self.host = Some(h.to_owned());
        self
    }
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut m = Hmac::<Sha256>::new_from_slice(key).unwrap();
    m.update(data);
    m.finalize().into_bytes().to_vec()
}

pub fn amz_date(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    // Days since 1970-01-01 to civil date (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

pub fn signing_key(date: &str) -> Vec<u8> {
    let k = hmac(format!("AWS4{SECRET}").as_bytes(), &date.as_bytes()[..8]);
    let k = hmac(&k, REGION.as_bytes());
    let k = hmac(&k, b"s3");
    hmac(&k, b"aws4_request")
}

pub fn credential(date: &str) -> String {
    format!("{ACCESS}/{}/{REGION}/s3/aws4_request", &date[..8])
}

pub fn sign_string(date: &str, string_to_sign: &str) -> String {
    hex::encode(hmac(&signing_key(date), string_to_sign.as_bytes()))
}

pub async fn send(addr: SocketAddr, r: Req<'_>) -> Resp {
    let host = r.host.clone().unwrap_or_else(|| addr.to_string());
    let payload_hash = hex::encode(Sha256::digest(r.body));
    let date = amz_date(SystemTime::now());
    let mut headers: Vec<(String, String)> = r.headers.iter().map(|(k, v)| (k.to_ascii_lowercase(), (*v).to_owned())).collect();
    headers.push(("host".into(), host.clone()));
    if r.chunked {
        headers.push(("transfer-encoding".into(), "chunked".into()));
    }
    if r.signed {
        headers.push(("x-amz-content-sha256".into(), payload_hash.clone()));
        if r.date_header {
            // The zone as botocore writes it.
            headers.push(("date".into(), http_date(SystemTime::now()).replace("GMT", "-0000")));
        } else {
            headers.push(("x-amz-date".into(), date.clone()));
        }
        headers.sort();
        let signed_names: Vec<&str> = headers.iter().map(|(k, _)| k.as_str()).collect();
        let canonical_headers: String = headers.iter().map(|(k, v)| format!("{k}:{}\n", v.trim())).collect();
        let mut query: Vec<&str> = r.query.split('&').filter(|s| !s.is_empty()).collect();
        query.sort();
        let canonical_query: Vec<String> = query
            .iter()
            .map(|q| if q.contains('=') { q.to_string() } else { format!("{q}=") })
            .collect();
        let canonical = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            r.method,
            r.path,
            canonical_query.join("&"),
            canonical_headers,
            signed_names.join(";"),
            payload_hash
        );
        let scope = format!("{}/{REGION}/s3/aws4_request", &date[..8]);
        let sts = format!(
            "AWS4-HMAC-SHA256\n{date}\n{scope}\n{}",
            hex::encode(Sha256::digest(canonical.as_bytes()))
        );
        let sig = sign_string(&date, &sts);
        headers.push((
            "authorization".into(),
            format!(
                "AWS4-HMAC-SHA256 Credential={ACCESS}/{scope}, SignedHeaders={}, Signature={sig}",
                signed_names.join(";")
            ),
        ));
    }
    let target = if r.query.is_empty() {
        r.path.to_owned()
    } else {
        format!("{}?{}", r.path, r.query)
    };
    let mut raw = format!("{} {target} HTTP/1.1\r\n", r.method).into_bytes();
    for (k, v) in &headers {
        raw.extend_from_slice(format!("{k}: {v}\r\n").as_bytes());
    }
    if r.chunked {
        raw.extend_from_slice(b"connection: close\r\n\r\n");
        // Two chunks, to show the body is reassembled.
        let (a, b) = r.body.split_at(r.body.len() / 2);
        for part in [a, b] {
            if !part.is_empty() {
                raw.extend_from_slice(format!("{:x}\r\n", part.len()).as_bytes());
                raw.extend_from_slice(part);
                raw.extend_from_slice(b"\r\n");
            }
        }
        raw.extend_from_slice(b"0\r\n\r\n");
    } else {
        raw.extend_from_slice(format!("content-length: {}\r\nconnection: close\r\n\r\n", r.body.len()).as_bytes());
        raw.extend_from_slice(r.body);
    }

    send_raw(addr, &raw).await
}

/// Sends `raw` as it is (a complete request, `Connection: close`) and reads the response.
pub async fn send_raw(addr: SocketAddr, raw: &[u8]) -> Resp {
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(raw).await.unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).await.unwrap();
    parse(&buf)
}

fn parse(buf: &[u8]) -> Resp {
    let split = buf.windows(4).position(|w| w == b"\r\n\r\n").expect("no header terminator") + 4;
    let head = String::from_utf8_lossy(&buf[..split]).into_owned();
    let mut lines = head.lines();
    let status = lines.next().unwrap().split(' ').nth(1).unwrap().parse().unwrap();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect();
    let mut body = buf[split..].to_vec();
    if headers
        .iter()
        .any(|(k, v)| k.eq_ignore_ascii_case("transfer-encoding") && v.eq_ignore_ascii_case("chunked"))
    {
        body = dechunk(&body);
    }
    Resp {
        status,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    }
}

fn dechunk(mut b: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let eol = b.windows(2).position(|w| w == b"\r\n").unwrap();
        let n = usize::from_str_radix(std::str::from_utf8(&b[..eol]).unwrap().trim(), 16).unwrap();
        if n == 0 {
            return out;
        }
        out.extend_from_slice(&b[eol + 2..eol + 2 + n]);
        b = &b[eol + 2 + n + 2..];
    }
}

/// `Wed, 02 Jan 2030 03:04:05 GMT`.
pub fn http_date(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).unwrap().as_secs();
    let days = secs / 86400;
    let iso = amz_date(t);
    let month =
        ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"][iso[4..6].parse::<usize>().unwrap() - 1];
    let weekday = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"][(days % 7) as usize];
    format!(
        "{weekday}, {} {month} {} {}:{}:{} GMT",
        &iso[6..8],
        &iso[0..4],
        &iso[9..11],
        &iso[11..13],
        &iso[13..15]
    )
}

/// Base64 HMAC-SHA1 of `data`: a SigV2 signature.
pub fn v2_signature(data: &str) -> String {
    use hmac::{Hmac, KeyInit, Mac};
    let mut m = Hmac::<sha1::Sha1>::new_from_slice(SECRET.as_bytes()).unwrap();
    m.update(data.as_bytes());
    base64_simd::STANDARD.encode_to_string(m.finalize().into_bytes())
}

/// The SigV2 `Authorization` header value for a request. `resource` is the CanonicalizedResource
/// (`/bkt/`, `/bkt/key?acl`), `date` goes into the Date slot (empty when `x-amz-date` is sent) and
/// `amz` are the `x-amz-*` headers sent, in any order.
pub fn v2_authorization(method: &str, content_type: &str, date: &str, amz: &[(&str, &str)], resource: &str) -> String {
    let mut amz: Vec<_> = amz.iter().map(|(k, v)| (k.to_ascii_lowercase(), v.trim())).collect();
    amz.sort();
    let amz: String = amz.iter().map(|(k, v)| format!("{k}:{v}\n")).collect();
    format!(
        "AWS {ACCESS}:{}",
        v2_signature(&format!("{method}\n\n{content_type}\n{date}\n{amz}{resource}"))
    )
}
