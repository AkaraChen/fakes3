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
    if r.signed {
        headers.push(("x-amz-content-sha256".into(), payload_hash.clone()));
        headers.push(("x-amz-date".into(), date.clone()));
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
    raw.extend_from_slice(format!("content-length: {}\r\nconnection: close\r\n\r\n", r.body.len()).as_bytes());
    raw.extend_from_slice(r.body);

    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(&raw).await.unwrap();
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
