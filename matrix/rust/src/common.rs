use std::collections::hash_map::RandomState;
use std::fmt;
use std::future::Future;
use std::hash::{BuildHasher, Hasher};
use std::time::{Duration, Instant};

use aws_sdk_s3::config::{BehaviorVersion, Credentials, Region};
use aws_sdk_s3::error::ProvideErrorMetadata;
use md5::{Digest, Md5};
use serde_json::{json, Value};

pub const SCENARIO_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Debug)]
pub struct Env {
    pub endpoint: String,
    pub ak: String,
    pub sk: String,
    pub region: String,
    pub server: String,
    pub out: String,
}

impl Env {
    pub fn load() -> Self {
        let g = |k: &str, d: &str| std::env::var(k).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| d.to_string());
        let server = g("MATRIX_SERVER", "fakes3");
        Env {
            endpoint: g("S3_ENDPOINT", "http://127.0.0.1:9100"),
            ak: g("S3_ACCESS_KEY", "fakes3"),
            sk: g("S3_SECRET_KEY", "fakes3secret"),
            region: g("S3_REGION", "us-east-1"),
            out: g("MATRIX_OUT", &format!("matrix/results/{server}")),
            server,
        }
    }
}

/// Marker error: the SDK does not offer the capability.
#[derive(Debug)]
pub struct Skip(pub String);
impl fmt::Display for Skip {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Skip {}

pub fn skip(reason: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(Skip(reason.into()))
}

#[macro_export]
macro_rules! check {
    ($cond:expr, $($arg:tt)+) => {
        if !$cond {
            return Err(anyhow::anyhow!("AssertionError: {}", format!($($arg)+)));
        }
    };
}

pub fn first_line_trunc(s: &str) -> String {
    let line = s.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    line.chars().take(300).collect()
}

pub struct Report {
    sdk: String,
    version: String,
    results: Vec<Value>,
}

impl Report {
    pub fn new(sdk: &str, version: &str) -> Self {
        Report { sdk: sdk.into(), version: version.into(), results: vec![] }
    }

    pub async fn run<F>(&mut self, scenario: &str, fut: F)
    where
        F: Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        let start = Instant::now();
        let res = tokio::spawn(tokio::time::timeout(SCENARIO_TIMEOUT, fut)).await;
        let ms = (start.elapsed().as_secs_f64() * 1000.0 * 100.0).round() / 100.0;
        let entry = match res {
            Ok(Ok(Ok(()))) => json!({"scenario": scenario, "status": "pass", "ms": ms}),
            Ok(Ok(Err(e))) => {
                if let Some(s) = e.downcast_ref::<Skip>() {
                    json!({"scenario": scenario, "status": "skip", "error": first_line_trunc(&s.0)})
                } else {
                    json!({"scenario": scenario, "status": "fail", "ms": ms, "error": first_line_trunc(&format!("{e:#}"))})
                }
            }
            Ok(Err(_)) => json!({"scenario": scenario, "status": "fail", "ms": ms, "error": "Timeout: scenario exceeded 60s"}),
            Err(je) => {
                let msg = if je.is_panic() {
                    let p = je.into_panic();
                    let s = p
                        .downcast_ref::<String>()
                        .cloned()
                        .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                        .unwrap_or_else(|| "unknown panic".into());
                    format!("Panic: {s}")
                } else {
                    format!("JoinError: {je}")
                };
                json!({"scenario": scenario, "status": "fail", "ms": ms, "error": first_line_trunc(&msg)})
            }
        };
        eprintln!(
            "  [{}] {:<28} {}{}",
            self.sdk,
            scenario,
            entry["status"].as_str().unwrap(),
            entry.get("error").and_then(|e| e.as_str()).map(|e| format!("  ({e})")).unwrap_or_default()
        );
        self.results.push(entry);
    }

    pub fn write(&self, env: &Env) -> anyhow::Result<()> {
        std::fs::create_dir_all(&env.out)?;
        let doc = json!({
            "sdk": self.sdk,
            "lang": "rust",
            "version": self.version,
            "server": env.server,
            "results": self.results,
        });
        let path = format!("{}/{}.json", env.out, self.sdk);
        std::fs::write(&path, serde_json::to_string_pretty(&doc)? + "\n")?;
        let count = |s: &str| self.results.iter().filter(|r| r["status"] == s).count();
        eprintln!("{}: pass={} fail={} skip={} -> {}", self.sdk, count("pass"), count("fail"), count("skip"), path);
        Ok(())
    }
}

pub fn rand_u64() -> u64 {
    let mut h = RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    h.finish()
}

pub fn rand_hex8() -> String {
    format!("{:016x}", rand_u64())[..8].to_string()
}

pub fn bucket_name(sdk: &str) -> String {
    format!("m-{}-{}", sdk.replace('_', "-"), rand_hex8())
}

pub fn rand_bytes(n: usize) -> Vec<u8> {
    let mut x = rand_u64() | 1;
    let mut v = Vec::with_capacity(n + 8);
    while v.len() < n {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        v.extend_from_slice(&x.to_le_bytes());
    }
    v.truncate(n);
    v
}

pub fn md5_hex(data: &[u8]) -> String {
    hex::encode(Md5::digest(data))
}

pub fn unquote(s: &str) -> String {
    s.trim().trim_matches('"').to_string()
}

pub fn is_multipart_etag(etag: &str, parts: usize) -> bool {
    let e = unquote(etag);
    match e.split_once('-') {
        Some((h, n)) => h.len() == 32 && h.chars().all(|c| c.is_ascii_hexdigit()) && n == parts.to_string(),
        None => false,
    }
}

pub const SPECIAL_KEYS: [&str; 3] = ["sp ace/üñí©ødé/a+b=c&d?e#f.txt", "a//b", "~tilde!*'()"];

pub fn range_body() -> Vec<u8> {
    (0..100u8).collect()
}

// ---------- aws-sdk-s3 helpers (also used for bucket setup by other SDKs) ----------

pub fn aws_client(env: &Env, secret: &str) -> aws_sdk_s3::Client {
    let creds = Credentials::new(&env.ak, secret, None, None, "matrix");
    let conf = aws_sdk_s3::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new(env.region.clone()))
        .endpoint_url(&env.endpoint)
        .credentials_provider(creds)
        .force_path_style(true)
        .build();
    aws_sdk_s3::Client::from_conf(conf)
}

/// Format an aws-sdk error as "Code: message (HTTP nnn)".
pub fn aerr<E>(e: aws_sdk_s3::error::SdkError<E, aws_smithy_runtime_api::http::Response>) -> anyhow::Error
where
    E: ProvideErrorMetadata + std::error::Error + Send + Sync + 'static,
{
    anyhow::anyhow!("{}", describe_aws(&e))
}

pub fn describe_aws<E>(e: &aws_sdk_s3::error::SdkError<E, aws_smithy_runtime_api::http::Response>) -> String
where
    E: ProvideErrorMetadata + std::error::Error + Send + Sync + 'static,
{
    let status = e.raw_response().map(|r| r.status().as_u16());
    match (e.code(), status) {
        (Some(code), st) => format!(
            "{}: {}{}",
            code,
            e.message().unwrap_or(""),
            st.map(|s| format!(" (HTTP {s})")).unwrap_or_default()
        ),
        (None, Some(st)) => format!("HttpError: HTTP {st} {}", aws_sdk_s3::error::DisplayErrorContext(e)),
        (None, None) => format!("SdkError: {}", aws_sdk_s3::error::DisplayErrorContext(e)),
    }
}

/// Assert that `r` is an error whose HTTP status or error code is among the expected values.
pub fn expect_aws_err<T, E>(
    r: Result<T, aws_sdk_s3::error::SdkError<E, aws_smithy_runtime_api::http::Response>>,
    statuses: &[u16],
    codes: &[&str],
) -> anyhow::Result<()>
where
    E: ProvideErrorMetadata + std::error::Error + Send + Sync + 'static,
{
    match r {
        Ok(_) => anyhow::bail!("AssertionError: expected error {codes:?}/{statuses:?}, got success"),
        Err(e) => {
            let st = e.raw_response().map(|r| r.status().as_u16());
            let ok = st.map(|s| statuses.contains(&s)).unwrap_or(false)
                || e.code().map(|c| codes.contains(&c)).unwrap_or(false);
            if ok {
                Ok(())
            } else {
                anyhow::bail!("AssertionError: expected {codes:?}/{statuses:?}, got {}", describe_aws(&e))
            }
        }
    }
}

pub async fn aws_create_bucket(c: &aws_sdk_s3::Client, b: &str) -> anyhow::Result<()> {
    c.create_bucket().bucket(b).send().await.map_err(aerr)?;
    Ok(())
}

/// Remove every object version, delete marker and pending upload in a bucket.
pub async fn aws_empty_bucket(c: &aws_sdk_s3::Client, b: &str) -> anyhow::Result<()> {
    // 1) every object version and delete marker (versioned buckets)
    if let Ok(v) = c.list_object_versions().bucket(b).send().await {
        let mut ids: Vec<(String, Option<String>)> = vec![];
        for o in v.versions() {
            ids.push((o.key().unwrap_or_default().into(), o.version_id().map(String::from)));
        }
        for o in v.delete_markers() {
            ids.push((o.key().unwrap_or_default().into(), o.version_id().map(String::from)));
        }
        for (k, v) in ids {
            let _ = c.delete_object().bucket(b).key(k).set_version_id(v).send().await;
        }
    }
    // 2) whatever is still listed (unversioned buckets, servers without ListObjectVersions)
    for _ in 0..50 {
        let r = c.list_objects_v2().bucket(b).send().await.map_err(aerr)?;
        if r.contents().is_empty() {
            break;
        }
        for o in r.contents() {
            c.delete_object().bucket(b).key(o.key().unwrap_or_default()).send().await.map_err(aerr)?;
        }
    }
    if let Ok(u) = c.list_multipart_uploads().bucket(b).send().await {
        for up in u.uploads() {
            let _ = c
                .abort_multipart_upload()
                .bucket(b)
                .key(up.key().unwrap_or_default())
                .upload_id(up.upload_id().unwrap_or_default())
                .send()
                .await;
        }
    }
    Ok(())
}

pub async fn aws_destroy_bucket(c: &aws_sdk_s3::Client, b: &str) {
    if c.head_bucket().bucket(b).send().await.is_err() {
        return;
    }
    let _ = aws_empty_bucket(c, b).await;
    let _ = c.delete_bucket().bucket(b).send().await;
}

pub fn http() -> reqwest::Client {
    reqwest::Client::builder().timeout(Duration::from_secs(55)).build().unwrap()
}
