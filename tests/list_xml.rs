//! Differential test for the direct list renderer (`src/listxml.rs`): two in-process servers, one
//! with `fast_list` on and one off, get the same randomized operations and the same list requests,
//! and every response must match: status, headers (but the request ids and the date) and body,
//! byte for byte. Version ids are random UUIDs, so those, and only those, are mapped to numbers in
//! order of first appearance before comparing.
//!
//! `FAKES3_LIST_SEED` picks the random sequence and `FAKES3_LIST_SCALE` multiplies the number of
//! requests (CI runs a larger scale; a failure prints the seed and the request).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, UNIX_EPOCH};

use fakes3::{Config, Server};

mod common;
use common::{Resp, req, send};

struct Rng(u64);

impl Rng {
    /// splitmix64.
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn one_in(&mut self, n: usize) -> bool {
        self.below(n) == 0
    }

    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len())]
    }
}

fn env_num(name: &str, default: u64) -> u64 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

// ---- keys and escaping -------------------------------------------------------------------------

const SEGS: &[&str] = &[
    "a",
    "b",
    "c",
    "dir",
    "sub",
    "x y",
    "日本語",
    "é",
    "😀",
    "+",
    "&",
    "<>",
    "\"'",
    "a&b",
    "%2F",
    "%",
    "~",
    "*",
    ".",
    "..",
    "-",
    "_",
    "tab\t",
    "nl\n",
    "cr\r",
    "]]>",
    "<![CDATA[",
    "&amp;",
    "文件",
    "Ünï",
    "0",
    "1",
    "10",
    "100",
    "\u{7f}",
    "\u{a0}",
    "\u{feff}",
];

fn gen_key(rng: &mut Rng, ctl: bool) -> String {
    let mut k = String::new();
    for i in 0..1 + rng.below(4) {
        if i > 0 {
            k.push_str(if rng.one_in(12) { "//" } else { "/" });
        }
        k.push_str(rng.pick(SEGS));
        if rng.one_in(3) {
            k.push_str(&rng.below(100).to_string());
        }
    }
    if rng.one_in(40) {
        let want = 600 + rng.below(400);
        while k.len() < want {
            k.push_str(rng.pick(SEGS));
        }
        while k.len() > 1000 {
            k.pop();
        }
    }
    if rng.one_in(20) {
        k.push('/');
    }
    if ctl && rng.one_in(2) {
        let c = *rng.pick(&['\u{1}', '\u{8}', '\u{b}', '\u{1f}']);
        if rng.one_in(2) {
            k.insert(0, c);
        } else {
            k.push(c);
        }
    }
    k
}

fn pct(s: &str, keep_slash: bool) -> String {
    let mut out = String::new();
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') || (keep_slash && b == b'/') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn unpct(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&apos;", "'")
        .replace("&quot;", "\"")
        .replace("&#13;", "\r")
        .replace("&amp;", "&")
}

fn tags(body: &str, name: &str) -> Vec<String> {
    let (open, close) = (format!("<{name}>"), format!("</{name}>"));
    let mut out = Vec::new();
    let mut rest = body;
    while let Some(i) = rest.find(&open) {
        let tail = &rest[i + open.len()..];
        let j = tail.find(&close).expect("unterminated element");
        out.push(unescape(&tail[..j]));
        rest = &tail[j + close.len()..];
    }
    out
}

// ---- one request, two servers ------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
enum Auth {
    Signed,
    Anon,
    Unverified,
}

#[derive(Debug)]
struct Op {
    method: &'static str,
    path: String,
    query: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    auth: Auth,
}

fn op(method: &'static str, path: String) -> Op {
    Op {
        method,
        path,
        query: String::new(),
        headers: Vec::new(),
        body: Vec::new(),
        auth: Auth::Signed,
    }
}

struct Side {
    server: Server,
    addr: SocketAddr,
    /// Version ids in order of first appearance; a number is an index into it.
    ids: Vec<String>,
    seen: HashMap<String, usize>,
}

impl Side {
    async fn start(fast_list: bool) -> Self {
        let server = Server::start(Config {
            fast_list,
            request_log: false,
            ..Config::default()
        })
        .await
        .unwrap();
        server
            .clock()
            .freeze(Some(UNIX_EPOCH + Duration::from_secs(1_893_456_000)))
            .unwrap();
        Self {
            addr: server.addr(),
            server,
            ids: Vec::new(),
            seen: HashMap::new(),
        }
    }

    fn canon(&mut self, id: &str) -> String {
        if id == "null" {
            return id.to_owned();
        }
        let n = match self.seen.get(id) {
            Some(&n) => n,
            None => {
                self.ids.push(id.to_owned());
                self.seen.insert(id.to_owned(), self.ids.len() - 1);
                self.ids.len() - 1
            }
        };
        format!("V{n}")
    }

    /// Replaces `\u{e000}N\u{e000}` with the Nth version id this server has handed out.
    fn resolve(&self, s: &str) -> String {
        let mut out = String::new();
        let mut rest = s;
        while let Some(i) = rest.find('\u{e000}') {
            out.push_str(&rest[..i]);
            let tail = &rest[i + '\u{e000}'.len_utf8()..];
            let j = tail.find('\u{e000}').expect("unterminated placeholder");
            out.push_str(&self.ids[tail[..j].parse::<usize>().unwrap()]);
            rest = &tail[j + '\u{e000}'.len_utf8()..];
        }
        out.push_str(rest);
        out
    }

    fn canon_body(&mut self, body: &str) -> String {
        const TAGS: [&str; 3] = ["<VersionId>", "<NextVersionIdMarker>", "<VersionIdMarker>"];
        let mut out = String::with_capacity(body.len());
        let mut rest = body;
        while let Some((i, t)) = TAGS.iter().filter_map(|t| rest.find(t).map(|i| (i, *t))).min_by_key(|(i, _)| *i) {
            let start = i + t.len();
            out.push_str(&rest[..start]);
            let end = rest[start..].find('<').unwrap();
            let id = &rest[start..start + end];
            if !id.is_empty() {
                out.push_str(&self.canon(id));
            }
            rest = &rest[start + end..];
        }
        out.push_str(rest);
        out
    }

    async fn send(&mut self, op: &Op) -> Resp {
        let (path, query) = (self.resolve(&op.path), self.resolve(&op.query));
        let mut r = req(op.method, &path).query(&query).body(&op.body);
        for (k, v) in &op.headers {
            r = r.header(k, v);
        }
        r = match op.auth {
            Auth::Signed => r,
            Auth::Anon => r.anon(),
            Auth::Unverified => r.unverified(),
        };
        let mut resp = send(self.addr, r).await;
        for (k, v) in &mut resp.headers {
            if k.eq_ignore_ascii_case("x-amz-version-id") {
                *v = self.canon(v);
            }
        }
        resp.body = self.canon_body(&resp.body.clone());
        if resp.status >= 400 {
            // Error bodies carry the request's own ids.
            for t in ["RequestId", "HostId"] {
                if let Some(i) = resp.body.find(&format!("<{t}>")) {
                    let (from, to) = (i + t.len() + 2, resp.body.find(&format!("</{t}>")).unwrap());
                    resp.body.replace_range(from..to, "-");
                }
            }
        }
        resp
    }
}

fn first_difference(a: &str, b: &str) -> String {
    let at = a.bytes().zip(b.bytes()).position(|(x, y)| x != y).unwrap_or(a.len().min(b.len()));
    let around = |s: &str| {
        let lo = (0..=at.saturating_sub(80)).rev().find(|&i| s.is_char_boundary(i)).unwrap_or(0);
        let hi = (at + 80..=s.len().min(at + 80)).find(|&i| s.is_char_boundary(i)).unwrap_or(s.len());
        s[lo..hi.max(lo)].to_owned()
    };
    format!("first difference at byte {at}\n  fast: ...{}\n  dto:  ...{}", around(a), around(b))
}

const VOLATILE: [&str; 3] = ["date", "x-amz-request-id", "x-amz-id-2"];

fn visible_headers(r: &Resp) -> Vec<(String, String)> {
    r.headers
        .iter()
        .filter(|(k, _)| !VOLATILE.iter().any(|v| k.eq_ignore_ascii_case(v)))
        .map(|(k, v)| (k.to_ascii_lowercase(), v.clone()))
        .collect()
}

struct Pair {
    fast: Side,
    slow: Side,
    rng: Rng,
    seed: u64,
    compared: usize,
    /// Successful list responses that carried entries, to show the test isn't comparing empties.
    nonempty: usize,
    /// List responses by the fast server that were 200 and 500: the counters must account for them.
    ok_lists: u64,
    failed_lists: u64,
    /// How many successful bodies contained each of `FEATURES`.
    features: Vec<usize>,
}

/// What the generated lists have to exercise at least once.
const FEATURES: &[&str] = &[
    "<Owner><DisplayName>fakes3</DisplayName><ID>fakes3</ID></Owner>",
    "<RestoreStatus><IsRestoreInProgress>true</IsRestoreInProgress></RestoreStatus>",
    "<RestoreStatus><IsRestoreInProgress>false</IsRestoreInProgress><RestoreExpiryDate>",
    "<DeleteMarker>",
    "<IsLatest>false</IsLatest>",
    "<IsLatest>true</IsLatest>",
    "<VersionId>null</VersionId>",
    "<NextVersionIdMarker>",
    "<NextKeyMarker>",
    "<NextContinuationToken>",
    "<NextMarker>",
    "<ContinuationToken>",
    "<StartAfter>",
    "<Delimiter>",
    "<CommonPrefixes><Prefix>",
    "<EncodingType>url</EncodingType>",
    "%E6%97%A5",
    "&amp;",
    "&lt;",
    "&apos;",
    "&quot;",
    "&#13;",
    "<StorageClass>GLACIER</StorageClass>",
    "<StorageClass>DEEP_ARCHIVE</StorageClass>",
    "<StorageClass>STANDARD_IA</StorageClass>",
    "<StorageClass>STANDARD</StorageClass>",
    "<IsTruncated>true</IsTruncated>",
    "<IsTruncated>false</IsTruncated>",
    "<KeyCount>0</KeyCount>",
    "<Marker></Marker>",
    "<MaxKeys>1000</MaxKeys>",
    "日本語",
    "\t",
    "\n",
];

impl Pair {
    /// Sends `op` to both servers and insists the responses are the same; returns the fast one.
    async fn both(&mut self, op: &Op) -> Resp {
        let a = self.fast.send(op).await;
        let b = self.slow.send(op).await;
        self.compared += 1;
        let ctx = || format!("seed {}, request #{}: {op:#?}", self.seed, self.compared);
        assert_eq!(a.status, b.status, "status\n{}\n{}", ctx(), a.body);
        assert_eq!(visible_headers(&a), visible_headers(&b), "headers\n{}", ctx());
        assert!(a.body == b.body, "body\n{}\n{}", ctx(), first_difference(&a.body, &b.body));
        a
    }

    async fn advance(&mut self, nanos: i64) {
        self.fast.server.clock().advance(nanos).unwrap();
        self.slow.server.clock().advance(nanos).unwrap();
    }

    async fn plain(&mut self, method: &'static str, path: &str, query: &str, headers: &[(&str, &str)], body: &[u8]) -> Resp {
        let mut o = op(method, path.to_owned());
        o.query = query.to_owned();
        o.headers = headers.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect();
        o.body = body.to_vec();
        self.both(&o).await
    }
}

// ---- populating a bucket -----------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Plain,
    Versioned,
    Suspended,
    Archive,
    Control,
    Public,
    Big,
}

const CLASSES: &[&str] = &[
    "STANDARD",
    "REDUCED_REDUNDANCY",
    "STANDARD_IA",
    "ONEZONE_IA",
    "GLACIER",
    "GLACIER_IR",
    "DEEP_ARCHIVE",
    "INTELLIGENT_TIERING",
];
const ARCHIVE_CLASSES: &[&str] = &[
    "GLACIER",
    "GLACIER",
    "DEEP_ARCHIVE",
    "INTELLIGENT_TIERING",
    "STANDARD",
    "STANDARD_IA",
];

const VERSIONING: &str = "<VersioningConfiguration><Status>Enabled</Status></VersioningConfiguration>";
const SUSPENDED: &str = "<VersioningConfiguration><Status>Suspended</Status></VersioningConfiguration>";
const CORS: &str = "<CORSConfiguration><CORSRule><AllowedOrigin>*</AllowedOrigin><AllowedMethod>GET</AllowedMethod>\
    <ExposeHeader>ETag</ExposeHeader></CORSRule></CORSConfiguration>";
const TIERING: &str = "<IntelligentTieringConfiguration><Id>cfg</Id><Status>Enabled</Status>\
    <Tiering><Days>90</Days><AccessTier>ARCHIVE_ACCESS</AccessTier></Tiering>\
    <Tiering><Days>180</Days><AccessTier>DEEP_ARCHIVE_ACCESS</AccessTier></Tiering></IntelligentTieringConfiguration>";

struct Bucket {
    name: &'static str,
    kind: Kind,
    keys: Vec<String>,
}

async fn populate(p: &mut Pair, name: &'static str, kind: Kind, n: usize) -> Bucket {
    let mut b = Bucket {
        name,
        kind,
        keys: Vec::new(),
    };
    let path = format!("/{name}");
    let acl: &[(&str, &str)] = if kind == Kind::Public {
        &[("x-amz-acl", "public-read")]
    } else {
        &[]
    };
    assert_eq!(p.plain("PUT", &path, "", acl, b"").await.status, 200);
    if matches!(kind, Kind::Versioned | Kind::Suspended) {
        assert_eq!(p.plain("PUT", &path, "versioning", &[], VERSIONING.as_bytes()).await.status, 200);
    }
    if kind == Kind::Plain {
        p.plain("PUT", &path, "cors", &[], CORS.as_bytes()).await;
    }
    if kind == Kind::Archive {
        p.plain("PUT", &path, "intelligent-tiering&id=cfg", &[], TIERING.as_bytes()).await;
    }
    let mut versions: Vec<(String, String)> = Vec::new();
    for i in 0..n {
        let nanos = p.rng.below(2_000_000_000) as i64;
        p.advance(nanos).await;
        let key = if kind == Kind::Big {
            format!("d{}/s{}/k{i}", i % 7, i % 13)
        } else if !b.keys.is_empty() && matches!(kind, Kind::Versioned | Kind::Suspended) && p.rng.below(5) < 2 {
            p.rng.pick(&b.keys.clone()).clone()
        } else {
            gen_key(&mut p.rng, kind == Kind::Control)
        };
        let class = match kind {
            Kind::Archive => Some(*p.rng.pick(ARCHIVE_CLASSES)),
            Kind::Big => None,
            _ if p.rng.one_in(6) => Some(*p.rng.pick(CLASSES)),
            _ => None,
        };
        let body: Vec<u8> = (0..if kind == Kind::Big { 0 } else { p.rng.below(40) })
            .map(|_| b'a' + p.rng.below(26) as u8)
            .collect();
        let mut o = op("PUT", format!("{path}/{}", pct(&key, true)));
        if let Some(c) = class {
            o.headers.push(("x-amz-storage-class".into(), c.into()));
        }
        o.body = body;
        let r = p.both(&o).await;
        if r.status == 200 {
            b.keys.push(key.clone());
            if let Some(v) = r.header("x-amz-version-id") {
                versions.push((key, v.to_owned()));
            }
        }
        if kind == Kind::Suspended && i == n / 2 {
            assert_eq!(p.plain("PUT", &path, "versioning", &[], SUSPENDED.as_bytes()).await.status, 200);
        }
    }
    // Deletes: delete markers in versioned buckets, plus some explicit version deletes.
    for _ in 0..n / 6 {
        if b.keys.is_empty() {
            break;
        }
        let key = p.rng.pick(&b.keys.clone()).clone();
        let o = op("DELETE", format!("{path}/{}", pct(&key, true)));
        p.both(&o).await;
    }
    for (key, v) in versions.iter().filter(|_| p.rng.one_in(8)).cloned().collect::<Vec<_>>() {
        if let Some(n) = v.strip_prefix('V') {
            let mut o = op("DELETE", format!("{path}/{}", pct(&key, true)));
            o.query = format!("versionId=\u{e000}{n}\u{e000}");
            p.both(&o).await;
        }
    }
    if kind == Kind::Archive {
        archive_states(p, &b).await;
    }
    b
}

/// Restores in waves, with the clock moving in between, so listings meet ongoing, restored and
/// expired restores and Intelligent-Tiering objects that have idled into an archive tier.
async fn archive_states(p: &mut Pair, b: &Bucket) {
    let path = format!("/{}", b.name);
    p.advance(95 * 86_400 * 1_000_000_000).await;
    for wave in 0..4 {
        for key in b.keys.clone() {
            if !p.rng.one_in(3) {
                continue;
            }
            let tier = *p.rng.pick(&["Expedited", "Standard", "Bulk"]);
            let days = 1 + p.rng.below(3);
            let body = format!(
                "<RestoreRequest><Days>{days}</Days><GlacierJobParameters><Tier>{tier}</Tier></GlacierJobParameters></RestoreRequest>"
            );
            let mut o = op("POST", format!("{path}/{}", pct(&key, true)));
            o.query = "restore".into();
            o.body = body.into_bytes();
            p.both(&o).await;
        }
        let nanos = [90, 4 * 3600, 2 * 86_400 + 1, 0][wave] * 1_000_000_000;
        let jitter = p.rng.below(1_000_000_000) as i64;
        p.advance(nanos + jitter).await;
    }
}

// ---- list requests -----------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Debug)]
enum Api {
    V1,
    V2,
    Versions,
}

fn query_of(api: Api, params: &[(String, String)]) -> String {
    let mut parts: Vec<String> = match api {
        Api::V1 => vec![],
        Api::V2 => vec!["list-type=2".into()],
        Api::Versions => vec!["versions".into()],
    };
    for (k, v) in params {
        // A version-id placeholder is already in query form.
        parts.push(if v.contains('\u{e000}') {
            format!("{k}={v}")
        } else {
            format!("{k}={}", pct(v, false))
        });
    }
    parts.join("&")
}

fn set(params: &mut Vec<(String, String)>, k: &str, v: String) {
    params.retain(|(pk, _)| pk != k);
    params.push((k.to_owned(), v));
}

fn gen_prefix(rng: &mut Rng, keys: &[String]) -> Option<String> {
    let known = |rng: &mut Rng| if keys.is_empty() { String::new() } else { rng.pick(keys).clone() };
    match rng.below(9) {
        0 => None,
        1 => Some(String::new()),
        2..=4 => {
            let k = known(rng);
            let cut = rng.below(k.chars().count() + 1);
            Some(k.chars().take(cut).collect())
        }
        5 => Some(format!("{}/", rng.pick(SEGS))),
        6 => Some(rng.pick(SEGS).to_string()),
        7 => Some(format!("{}/", known(rng))),
        _ => Some(format!("d{}/", rng.below(7))),
    }
}

async fn list_request(p: &mut Pair, b: &Bucket, stats: &mut Stats) {
    let archive = b.kind == Kind::Archive && p.rng.one_in(2);
    let api = if archive {
        Api::V2
    } else {
        *p.rng.pick(&[Api::V1, Api::V2, Api::V2, Api::Versions, Api::Versions])
    };
    let mut params: Vec<(String, String)> = Vec::new();
    if let Some(x) = gen_prefix(&mut p.rng, &b.keys) {
        params.push(("prefix".into(), x));
    }
    let delimiters = ["", "/", "/", "/", "//", "a", "/d", "%", "&", "é", "<", "x y", "1", "\n"];
    if !p.rng.one_in(3) {
        params.push(("delimiter".into(), p.rng.pick(&delimiters).to_string()));
    }
    let max_keys = [
        "0",
        "1",
        "2",
        "3",
        "5",
        "10",
        "50",
        "999",
        "1000",
        "1001",
        "5000",
        "-1",
        "x",
        "2147483648",
    ];
    if !p.rng.one_in(4) {
        let m = if b.kind == Kind::Big && p.rng.below(3) > 0 {
            "100"
        } else {
            p.rng.pick(&max_keys)
        };
        params.push(("max-keys".into(), m.to_string()));
    }
    match p.rng.below(8) {
        0..=2 => params.push(("encoding-type".into(), "url".into())),
        3 => params.push(("encoding-type".into(), p.rng.pick(&["URL", "Url", "foo", ""]).to_string())),
        _ => {}
    }
    if api == Api::V2 {
        match p.rng.below(3) {
            0 => params.push(("fetch-owner".into(), "true".into())),
            1 => params.push(("fetch-owner".into(), p.rng.pick(&["false", "x"]).to_string())),
            _ => {}
        }
    }
    // A starting point: sometimes a real key, sometimes noise.
    let start = |rng: &mut Rng| match rng.below(4) {
        0 => rng.pick(SEGS).to_string(),
        1 => String::new(),
        _ if b.keys.is_empty() => "a".into(),
        _ => rng.pick(&b.keys).clone(),
    };
    if p.rng.one_in(3) {
        match api {
            Api::V1 => params.push(("marker".into(), start(&mut p.rng))),
            Api::V2 => {
                if p.rng.one_in(2) {
                    params.push(("start-after".into(), start(&mut p.rng)));
                } else if p.rng.one_in(6) {
                    params.push(("continuation-token".into(), p.rng.pick(&["zz", "", "6162"]).to_string()));
                } else {
                    params.push((
                        "continuation-token".into(),
                        start(&mut p.rng).bytes().map(|c| format!("{c:02x}")).collect(),
                    ));
                }
            }
            Api::Versions => {
                params.push(("key-marker".into(), start(&mut p.rng)));
                if p.rng.one_in(3) {
                    params.push(("version-id-marker".into(), p.rng.pick(&["null", "nope", ""]).to_string()));
                }
            }
        }
    }
    let mut headers: Vec<(String, String)> = Vec::new();
    if api == Api::V2 && (archive || p.rng.one_in(3)) {
        let v = *p
            .rng
            .pick(&["RestoreStatus", "RestoreStatus", "restorestatus", "RestoreStatus, Other", "Other"]);
        headers.push(("x-amz-optional-object-attributes".into(), v.into()));
    }
    if p.rng.one_in(10) {
        headers.push(("origin".into(), "https://app.test".into()));
    }
    if p.rng.one_in(14) {
        headers.push((
            "x-amz-expected-bucket-owner".into(),
            p.rng.pick(&["fakes3", "123456789012"]).to_string(),
        ));
    }
    let auth = match p.rng.below(10) {
        0 => Auth::Anon,
        1 => Auth::Unverified,
        _ => Auth::Signed,
    };
    // Page through to the end (or a bounded number of pages).
    let paging = p.rng.below(3) != 0;
    for _ in 0..stats.pages {
        let mut o = op("GET", format!("/{}", b.name));
        o.query = query_of(api, &params);
        o.headers = headers.clone();
        o.auth = auth;
        let r = p.both(&o).await;
        stats.requests += 1;
        match r.status {
            200 => {
                p.ok_lists += 1;
                for (n, f) in p.features.iter_mut().zip(FEATURES) {
                    *n += usize::from(r.body.contains(f));
                }
                if r.body.contains("<Key>") || r.body.contains("<Prefix>") && r.body.contains("<CommonPrefixes>") {
                    p.nonempty += 1;
                }
            }
            500 => p.failed_lists += 1,
            _ => {}
        }
        if r.status != 200 || !paging || !r.body.contains("<IsTruncated>true</IsTruncated>") {
            break;
        }
        let url = params.iter().any(|(k, v)| k == "encoding-type" && v.eq_ignore_ascii_case("url"));
        let dec = |s: &str| if url { unpct(s) } else { s.to_owned() };
        match api {
            Api::V2 => {
                let Some(t) = tags(&r.body, "NextContinuationToken").pop() else {
                    break;
                };
                set(&mut params, "continuation-token", t);
            }
            Api::V1 => {
                let next = tags(&r.body, "NextMarker").pop().or_else(|| tags(&r.body, "Key").pop());
                let Some(m) = next else { break };
                set(&mut params, "marker", dec(&m));
            }
            Api::Versions => {
                let Some(k) = tags(&r.body, "NextKeyMarker").pop() else { break };
                set(&mut params, "key-marker", dec(&k));
                params.retain(|(pk, _)| pk != "version-id-marker");
                if let Some(v) = tags(&r.body, "NextVersionIdMarker").pop() {
                    let canon = v.strip_prefix('V').map(|n| format!("\u{e000}{n}\u{e000}")).unwrap_or(v);
                    params.push(("version-id-marker".into(), canon));
                }
            }
        }
        stats.paged += 1;
    }
}

struct Stats {
    pages: usize,
    requests: usize,
    paged: usize,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fast_and_dto_list_responses_are_identical() {
    let seed = env_num("FAKES3_LIST_SEED", 0x11_5EED);
    let scale = env_num("FAKES3_LIST_SCALE", 1) as usize;
    let (fast_before, fallback_before) = fakes3::list_fast_path_counters();
    let mut p = Pair {
        fast: Side::start(true).await,
        slow: Side::start(false).await,
        rng: Rng(seed),
        seed,
        compared: 0,
        nonempty: 0,
        ok_lists: 0,
        failed_lists: 0,
        features: vec![0; FEATURES.len()],
    };
    let mut stats = Stats {
        pages: 6,
        requests: 0,
        paged: 0,
    };
    let worlds: &[(&'static str, Kind, usize, usize)] = &[
        ("plain", Kind::Plain, 90, 140),
        ("ver", Kind::Versioned, 80, 160),
        ("susp", Kind::Suspended, 50, 60),
        ("arch", Kind::Archive, 70, 120),
        ("pub", Kind::Public, 25, 50),
        ("ctl", Kind::Control, 25, 60),
        ("big", Kind::Big, 1100, 25),
        ("empty", Kind::Plain, 0, 20),
    ];
    for &(name, kind, keys, requests) in worlds {
        let b = populate(&mut p, name, kind, keys).await;
        // Paging through the big bucket one key at a time would take forever.
        stats.pages = if kind == Kind::Big { 4 } else { 8 };
        for _ in 0..requests * scale {
            list_request(&mut p, &b, &mut stats).await;
        }
    }
    // A bucket that does not exist, and listings of every kind without any parameter.
    for api in [Api::V1, Api::V2, Api::Versions] {
        for bucket in ["nope", "plain", "ver", "arch", "big"] {
            let mut o = op("GET", format!("/{bucket}"));
            o.query = query_of(api, &[]);
            p.both(&o).await;
        }
    }
    let (fast, fallback) = fakes3::list_fast_path_counters();
    eprintln!(
        "seed {seed}: {} requests compared ({} lists, {} with entries, {} follow-up pages); fast path {}, fallback {}",
        p.compared,
        stats.requests,
        p.nonempty,
        stats.paged,
        fast - fast_before,
        fallback - fallback_before
    );
    assert!(
        p.nonempty * 4 >= stats.requests,
        "most lists should return entries ({} of {})",
        p.nonempty,
        stats.requests
    );
    let missing: Vec<_> = FEATURES
        .iter()
        .zip(&p.features)
        .filter(|(_, n)| **n == 0)
        .map(|(f, _)| *f)
        .collect();
    assert!(missing.is_empty(), "never listed: {missing:?}");
    assert!(stats.paged > 20, "paging was exercised");
    // The fast path really served the 200s; the 500s are the control characters s3s refuses to send.
    assert!(p.failed_lists > 0, "the control-character bucket produced no failures");
    assert_eq!(
        fallback - fallback_before,
        p.failed_lists,
        "every 500 is a fallback to the s3s path"
    );
    // The unparameterised lists at the end are not in `ok_lists`; there are 15 of them.
    let fast = fast - fast_before;
    assert!(
        (p.ok_lists..=p.ok_lists + 15).contains(&fast),
        "{fast} fast renders for {} successful lists",
        p.ok_lists
    );
}
