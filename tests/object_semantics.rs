//! Object Lock, SSE, storage class and GetObjectAttributes over real HTTP. A bare-bones HTTP/1.1
//! client keeps this free of SDK dependencies; the server runs unauthenticated.

use std::time::{Duration, UNIX_EPOCH};

use fakes3::{BucketDefaults, Config, Server};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

struct Resp {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Resp {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn code(&self) -> &str {
        tag(&self.body, "Code").unwrap_or("")
    }
}

/// Text of the first `<name>` element.
fn tag<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let start = xml.find(&format!("<{name}>"))? + name.len() + 2;
    let end = xml[start..].find(&format!("</{name}>"))?;
    Some(&xml[start..start + end])
}

struct Client(String);

impl Client {
    async fn call(&self, method: &str, path: &str, headers: &[(&str, &str)], body: &[u8]) -> Resp {
        let mut s = TcpStream::connect(&self.0).await.unwrap();
        let mut head = format!(
            "{method} {path} HTTP/1.1\r\nhost: {}\r\nconnection: close\r\ncontent-length: {}\r\n",
            self.0,
            body.len()
        );
        for (k, v) in headers {
            head.push_str(&format!("{k}: {v}\r\n"));
        }
        head.push_str("\r\n");
        s.write_all(head.as_bytes()).await.unwrap();
        s.write_all(body).await.unwrap();
        let mut raw = Vec::new();
        s.read_to_end(&mut raw).await.unwrap();
        let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let head = String::from_utf8_lossy(&raw[..split]).into_owned();
        let mut lines = head.lines();
        let status = lines.next().unwrap().split(' ').nth(1).unwrap().parse().unwrap();
        let headers: Vec<_> = lines
            .filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned())))
            .collect();
        let mut body = raw[split + 4..].to_vec();
        if headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("transfer-encoding") && v.contains("chunked"))
        {
            body = dechunk(&body);
        }
        Resp {
            status,
            headers,
            body: String::from_utf8_lossy(&body).into_owned(),
        }
    }

    async fn put(&self, path: &str, headers: &[(&str, &str)], body: &str) -> Resp {
        self.call("PUT", path, headers, body.as_bytes()).await
    }

    async fn get(&self, path: &str, headers: &[(&str, &str)]) -> Resp {
        self.call("GET", path, headers, b"").await
    }

    async fn head(&self, path: &str, headers: &[(&str, &str)]) -> Resp {
        self.call("HEAD", path, headers, b"").await
    }

    async fn delete(&self, path: &str, headers: &[(&str, &str)]) -> Resp {
        self.call("DELETE", path, headers, b"").await
    }

    /// Creates an Object Lock bucket.
    async fn lock_bucket(&self, name: &str) {
        let r = self
            .put(&format!("/{name}"), &[("x-amz-bucket-object-lock-enabled", "true")], "")
            .await;
        assert_eq!(r.status, 200, "{}", r.body);
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

async fn start() -> (Server, Client) {
    start_with(BucketDefaults::Legacy).await
}

async fn start_with(bucket_defaults: BucketDefaults) -> (Server, Client) {
    let server = Server::start(Config {
        auth: false,
        bucket_defaults,
        ..Config::default()
    })
    .await
    .unwrap();
    let c = Client(server.addr().to_string());
    (server, c)
}

const SSE_C_KEY: &str = "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY="; // "0123456789abcdef0123456789abcdef"
const SSE_C_MD5: &str = "hRasmdxgYDKV3nvbahU1MA==";
const OTHER_KEY: &str = "ZmVkY2JhOTg3NjU0MzIxMGZlZGNiYTk4NzY1NDMyMTA=";
const OTHER_MD5: &str = "dT1y7SEiqn6YCJ3QeqwoIw==";

fn sse_c<'a>(key: &'a str, md5: &'a str) -> [(&'a str, &'a str); 3] {
    [
        ("x-amz-server-side-encryption-customer-algorithm", "AES256"),
        ("x-amz-server-side-encryption-customer-key", key),
        ("x-amz-server-side-encryption-customer-key-md5", md5),
    ]
}

#[tokio::test]
async fn object_lock_governance_and_bypass() {
    let (server, c) = start().await;
    c.lock_bucket("bkt").await;
    let v = c.get("/bkt?versioning", &[]).await;
    assert!(v.body.contains("<Status>Enabled</Status>"), "{}", v.body);
    // Lock and versioning cannot be turned off again.
    let r = c
        .put(
            "/bkt?versioning",
            &[],
            "<VersioningConfiguration><Status>Suspended</Status></VersioningConfiguration>",
        )
        .await;
    assert_eq!((r.status, r.code()), (409, "InvalidBucketState"));

    let r = c
        .put(
            "/bkt/k",
            &[
                ("x-amz-object-lock-mode", "GOVERNANCE"),
                ("x-amz-object-lock-retain-until-date", "2099-01-01T00:00:00Z"),
            ],
            "data",
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.body);
    let vid = r.header("x-amz-version-id").unwrap().to_owned();
    let h = c.head("/bkt/k", &[]).await;
    assert_eq!(h.header("x-amz-object-lock-mode"), Some("GOVERNANCE"));
    assert!(
        h.header("x-amz-object-lock-retain-until-date")
            .unwrap()
            .starts_with("2099-01-01T00:00:00")
    );
    assert_eq!(h.header("x-amz-object-lock-legal-hold"), None);

    // Deleting without a version id only adds a delete marker.
    assert_eq!(c.delete("/bkt/k", &[]).await.status, 204);
    let path = format!("/bkt/k?versionId={vid}");
    let r = c.delete(&path, &[]).await;
    assert_eq!((r.status, r.code()), (403, "AccessDenied"));
    assert_eq!(c.delete(&path, &[("x-amz-bypass-governance-retention", "true")]).await.status, 204);
    assert_eq!(c.get(&path, &[]).await.code(), "NoSuchVersion");
    drop(server);
}

#[tokio::test]
async fn object_lock_compliance_cannot_shorten_and_expires_with_the_clock() {
    let (server, c) = start().await;
    c.lock_bucket("bkt").await;
    server
        .clock()
        .freeze(Some(UNIX_EPOCH + Duration::from_secs(1_700_000_000)))
        .unwrap(); // 2023-11-14
    let r = c
        .put(
            "/bkt/k",
            &[
                ("x-amz-object-lock-mode", "COMPLIANCE"),
                ("x-amz-object-lock-retain-until-date", "2023-12-01T00:00:00Z"),
            ],
            "x",
        )
        .await;
    let path = format!("/bkt/k?versionId={}", r.header("x-amz-version-id").unwrap());
    let bypass = [("x-amz-bypass-governance-retention", "true")];

    assert_eq!(c.delete(&path, &bypass).await.status, 403);
    let retention = |date: &str, mode: &str| format!("<Retention><Mode>{mode}</Mode><RetainUntilDate>{date}</RetainUntilDate></Retention>");
    let r = c
        .put(
            &format!("{path}&retention"),
            &bypass,
            &retention("2023-11-20T00:00:00Z", "COMPLIANCE"),
        )
        .await;
    assert_eq!((r.status, r.code()), (403, "AccessDenied"));
    let r = c
        .put(
            &format!("{path}&retention"),
            &bypass,
            &retention("2024-01-01T00:00:00Z", "GOVERNANCE"),
        )
        .await;
    assert_eq!(r.status, 403, "COMPLIANCE cannot become GOVERNANCE");
    let r = c
        .put(&format!("{path}&retention"), &[], &retention("2024-01-01T00:00:00Z", "COMPLIANCE"))
        .await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = c.get(&format!("{path}&retention"), &[]).await;
    assert!(
        r.body.contains("<Mode>COMPLIANCE</Mode>") && r.body.contains("2024-01-01T00:00:00"),
        "{}",
        r.body
    );

    // Still protected on 2023-12-15, free on 2024-01-02.
    server.clock().set(UNIX_EPOCH + Duration::from_secs(1_702_598_400)).unwrap();
    assert_eq!(c.delete(&path, &[]).await.status, 403);
    server.clock().set(UNIX_EPOCH + Duration::from_secs(1_704_153_600)).unwrap();
    assert_eq!(c.delete(&path, &[]).await.status, 204);
}

#[tokio::test]
async fn object_retention_api() {
    let (_server, c) = start().await;
    c.put("/plain", &[], "").await;
    c.put("/plain/k", &[], "x").await;
    let r = c
        .put(
            "/plain/k?retention",
            &[],
            "<Retention><Mode>GOVERNANCE</Mode><RetainUntilDate>2099-01-01T00:00:00Z</RetainUntilDate></Retention>",
        )
        .await;
    assert_eq!((r.status, r.code()), (400, "InvalidRequest"), "{}", r.body);
    let r = c.put("/plain/k", &[("x-amz-object-lock-legal-hold", "ON")], "x").await;
    assert_eq!((r.status, r.code()), (400, "InvalidRequest"));
    assert_eq!(
        c.get("/plain?object-lock", &[]).await.code(),
        "ObjectLockConfigurationNotFoundError"
    );

    c.lock_bucket("bkt").await;
    let r = c.put("/bkt/k", &[], "x").await;
    let path = format!("/bkt/k?versionId={}", r.header("x-amz-version-id").unwrap());
    let r = c.get(&format!("{path}&retention"), &[]).await;
    assert_eq!((r.status, r.code()), (404, "NoSuchObjectLockConfiguration"));
    let r = c.get(&format!("{path}&legal-hold"), &[]).await;
    assert_eq!((r.status, r.code()), (404, "NoSuchObjectLockConfiguration"));

    let past = "<Retention><Mode>GOVERNANCE</Mode><RetainUntilDate>2001-01-01T00:00:00Z</RetainUntilDate></Retention>";
    assert_eq!(c.put(&format!("{path}&retention"), &[], past).await.code(), "InvalidArgument");
    let r = c
        .put(&format!("{path}&retention"), &[], "<Retention><Mode>GOVERNANCE</Mode></Retention>")
        .await;
    assert_eq!(r.code(), "MalformedXML");
    let r = c.put("/bkt/k", &[("x-amz-object-lock-mode", "GOVERNANCE")], "x").await;
    assert_eq!(r.code(), "InvalidArgument");

    // Set, extend freely, shorten only with bypass, clear only with bypass.
    let set = |d: &str| format!("<Retention><Mode>GOVERNANCE</Mode><RetainUntilDate>{d}</RetainUntilDate></Retention>");
    assert_eq!(
        c.put(&format!("{path}&retention"), &[], &set("2098-01-01T00:00:00Z")).await.status,
        200
    );
    assert_eq!(
        c.put(&format!("{path}&retention"), &[], &set("2099-01-01T00:00:00Z")).await.status,
        200
    );
    assert_eq!(
        c.put(&format!("{path}&retention"), &[], &set("2098-06-01T00:00:00Z")).await.status,
        403
    );
    let bypass = [("x-amz-bypass-governance-retention", "true")];
    assert_eq!(
        c.put(&format!("{path}&retention"), &bypass, &set("2098-06-01T00:00:00Z"))
            .await
            .status,
        200
    );
    assert_eq!(
        c.put(&format!("{path}&retention"), &[], "<Retention></Retention>").await.status,
        403
    );
    assert_eq!(
        c.put(&format!("{path}&retention"), &bypass, "<Retention></Retention>").await.status,
        200
    );
    assert_eq!(c.get(&format!("{path}&retention"), &[]).await.status, 404);
}

#[tokio::test]
async fn legal_hold_blocks_delete_until_released() {
    let (_server, c) = start().await;
    c.lock_bucket("bkt").await;
    let r = c.put("/bkt/k", &[("x-amz-object-lock-legal-hold", "ON")], "x").await;
    let path = format!("/bkt/k?versionId={}", r.header("x-amz-version-id").unwrap());
    assert_eq!(c.head(&path, &[]).await.header("x-amz-object-lock-legal-hold"), Some("ON"));
    let r = c.get(&format!("{path}&legal-hold"), &[]).await;
    assert!(r.body.contains("<Status>ON</Status>"), "{}", r.body);

    let bypass = [("x-amz-bypass-governance-retention", "true")];
    assert_eq!(c.delete(&path, &bypass).await.status, 403, "bypass does not lift a legal hold");
    let off = "<LegalHold><Status>OFF</Status></LegalHold>";
    assert_eq!(
        c.put(&format!("{path}&legal-hold"), &[], "<LegalHold><Status>MAYBE</Status></LegalHold>")
            .await
            .code(),
        "MalformedXML"
    );
    assert_eq!(c.put(&format!("{path}&legal-hold"), &[], off).await.status, 200);
    assert_eq!(c.head(&path, &[]).await.header("x-amz-object-lock-legal-hold"), Some("OFF"));
    assert_eq!(c.delete(&path, &[]).await.status, 204);
}

#[tokio::test]
async fn default_retention_and_delete_objects() {
    let (server, c) = start().await;
    c.lock_bucket("bkt").await;
    let cfg =
        |rule: &str| format!("<ObjectLockConfiguration><ObjectLockEnabled>Enabled</ObjectLockEnabled>{rule}</ObjectLockConfiguration>");
    let rule = |inner: &str| format!("<Rule><DefaultRetention>{inner}</DefaultRetention></Rule>");
    for bad in [
        "<Mode>GOVERNANCE</Mode>",
        "<Mode>GOVERNANCE</Mode><Days>1</Days><Years>1</Years>",
        "<Days>1</Days>",
    ] {
        assert_eq!(
            c.put("/bkt?object-lock", &[], &cfg(&rule(bad))).await.code(),
            "MalformedXML",
            "{bad}"
        );
    }
    assert_eq!(
        c.put("/bkt?object-lock", &[], &cfg(&rule("<Mode>GOVERNANCE</Mode><Days>0</Days>")))
            .await
            .code(),
        "InvalidRetentionPeriod"
    );
    let r = c
        .put("/bkt?object-lock", &[], &cfg(&rule("<Mode>COMPLIANCE</Mode><Days>2</Days>")))
        .await;
    assert_eq!(r.status, 200, "{}", r.body);
    let g = c.get("/bkt?object-lock", &[]).await;
    assert!(
        g.body.contains("<Mode>COMPLIANCE</Mode>") && g.body.contains("<Days>2</Days>"),
        "{}",
        g.body
    );

    server
        .clock()
        .freeze(Some(UNIX_EPOCH + Duration::from_secs(1_700_000_000)))
        .unwrap();
    let a = c.put("/bkt/a", &[], "1").await;
    let a = a.header("x-amz-version-id").unwrap().to_owned();
    let h = c.head("/bkt/a", &[]).await;
    assert_eq!(h.header("x-amz-object-lock-mode"), Some("COMPLIANCE"));
    assert!(
        h.header("x-amz-object-lock-retain-until-date")
            .unwrap()
            .starts_with("2023-11-16T22:13:20"),
        "{h:?}",
        h = h.headers
    );
    let b = c
        .put(
            "/bkt/bkt",
            &[
                ("x-amz-object-lock-mode", "GOVERNANCE"),
                ("x-amz-object-lock-retain-until-date", "2023-11-20T00:00:00Z"),
            ],
            "2",
        )
        .await;
    let b = b.header("x-amz-version-id").unwrap().to_owned();
    // A copy does not inherit the source's lock, it gets the bucket default.
    let r = c.put("/bkt/c", &[("x-amz-copy-source", "/bkt/bkt")], "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let h = c.head("/bkt/c", &[]).await;
    assert_eq!(h.header("x-amz-object-lock-mode"), Some("COMPLIANCE"));

    // DeleteObjects reports each locked version as an error and still deletes the others.
    let free = c.put("/bkt/free", &[], "3").await;
    let free_vid = free.header("x-amz-version-id").unwrap().to_owned();
    c.put("/bkt?object-lock", &[], &cfg("")).await;
    let body = format!(
        "<Delete><Object><Key>a</Key><VersionId>{a}</VersionId></Object><Object><Key>b</Key><VersionId>{b}</VersionId></Object></Delete>"
    );
    let r = c
        .call(
            "POST",
            "/bkt?delete",
            &[("x-amz-bypass-governance-retention", "true")],
            body.as_bytes(),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(
        r.body.contains("<Error><Code>AccessDenied</Code>") || r.body.contains("<Code>AccessDenied</Code>"),
        "{}",
        r.body
    );
    assert_eq!(
        r.body.matches("<Deleted>").count(),
        1,
        "governance object deleted with bypass: {}",
        r.body
    );
    assert_eq!(c.head(&format!("/bkt/a?versionId={a}"), &[]).await.status, 200);
    let _ = free_vid;
}

#[tokio::test]
async fn sse_c_requires_the_same_key() {
    let (_server, c) = start().await;
    c.put("/bkt", &[], "").await;
    let r = c.put("/bkt/k", &sse_c(SSE_C_KEY, SSE_C_MD5), "secret").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.header("x-amz-server-side-encryption-customer-algorithm"), Some("AES256"));
    assert_eq!(r.header("x-amz-server-side-encryption-customer-key-md5"), Some(SSE_C_MD5));
    assert_ne!(
        r.header("etag").unwrap().trim_matches('"'),
        "5ebe2294ecd0e0f08eab7690d2a6ee69",
        "SSE-C ETags are not the content MD5"
    );

    let g = c.get("/bkt/k", &sse_c(SSE_C_KEY, SSE_C_MD5)).await;
    assert_eq!((g.status, g.body.as_str()), (200, "secret"));
    assert_eq!(g.header("x-amz-server-side-encryption-customer-key-md5"), Some(SSE_C_MD5));
    assert_eq!(c.head("/bkt/k", &sse_c(SSE_C_KEY, SSE_C_MD5)).await.status, 200);

    let r = c.get("/bkt/k", &[]).await;
    assert_eq!((r.status, r.code()), (400, "InvalidRequest"));
    assert_eq!(c.head("/bkt/k", &[]).await.status, 400);
    let r = c.get("/bkt/k", &sse_c(OTHER_KEY, OTHER_MD5)).await;
    assert_eq!((r.status, r.code()), (403, "AccessDenied"));
    // Malformed requests.
    let r = c.get("/bkt/k", &sse_c(SSE_C_KEY, OTHER_MD5)).await;
    assert_eq!((r.status, r.code()), (400, "InvalidArgument"));
    let r = c
        .put(
            "/bkt/x",
            &[
                ("x-amz-server-side-encryption-customer-algorithm", "AES128"),
                ("x-amz-server-side-encryption-customer-key", SSE_C_KEY),
                ("x-amz-server-side-encryption-customer-key-md5", SSE_C_MD5),
            ],
            "",
        )
        .await;
    assert_eq!(r.code(), "InvalidEncryptionAlgorithmError");
    let r = c
        .put("/bkt/x", &[("x-amz-server-side-encryption-customer-algorithm", "AES256")], "")
        .await;
    assert_eq!((r.status, r.code()), (400, "InvalidArgument"));
    let r = c
        .put(
            "/bkt/x",
            &[
                ("x-amz-server-side-encryption-customer-algorithm", "AES256"),
                ("x-amz-server-side-encryption-customer-key", "c2hvcnQ="),
                ("x-amz-server-side-encryption-customer-key-md5", SSE_C_MD5),
            ],
            "",
        )
        .await;
    assert_eq!(r.code(), "InvalidArgument");
    // Keys on an object that has none.
    c.put("/bkt/plain", &[], "p").await;
    let r = c.get("/bkt/plain", &sse_c(SSE_C_KEY, SSE_C_MD5)).await;
    assert_eq!((r.status, r.code()), (400, "InvalidRequest"));

    // CopyObject needs the source key, and the destination can pick its own.
    let r = c.put("/bkt/copy", &[("x-amz-copy-source", "/bkt/k")], "").await;
    assert_eq!((r.status, r.code()), (400, "InvalidRequest"));
    let mut h = vec![
        ("x-amz-copy-source", "/bkt/k"),
        ("x-amz-copy-source-server-side-encryption-customer-algorithm", "AES256"),
        ("x-amz-copy-source-server-side-encryption-customer-key", SSE_C_KEY),
        ("x-amz-copy-source-server-side-encryption-customer-key-md5", SSE_C_MD5),
    ];
    let r = c.put("/bkt/copy", &h, "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(c.get("/bkt/copy", &[]).await.body, "secret", "decrypted copy needs no key");
    h.extend(sse_c(OTHER_KEY, OTHER_MD5));
    assert_eq!(c.put("/bkt/copy2", &h, "").await.status, 200);
    assert_eq!(c.get("/bkt/copy2", &sse_c(OTHER_KEY, OTHER_MD5)).await.body, "secret");
}

#[tokio::test]
async fn sse_s3_kms_and_default_encryption() {
    let (_server, c) = start_with(BucketDefaults::Aws).await;
    c.put("/bkt", &[], "").await;
    // With the AWS defaults everything is SSE-S3 unless said otherwise.
    let r = c.put("/bkt/k", &[], "x").await;
    assert_eq!(r.header("x-amz-server-side-encryption"), Some("AES256"));
    assert_eq!(c.head("/bkt/k", &[]).await.header("x-amz-server-side-encryption"), Some("AES256"));
    assert_eq!(r.header("etag"), Some("\"9dd4e461268c8034f5c8564e155c67a6\""));
    let e = c.get("/bkt?encryption", &[]).await;
    assert!(e.body.contains("<SSEAlgorithm>AES256</SSEAlgorithm>"), "{}", e.body);

    let r = c
        .put(
            "/bkt/kms",
            &[
                ("x-amz-server-side-encryption", "aws:kms"),
                ("x-amz-server-side-encryption-aws-kms-key-id", "my-key"),
                ("x-amz-server-side-encryption-bucket-key-enabled", "true"),
            ],
            "x",
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.header("x-amz-server-side-encryption"), Some("aws:kms"));
    assert_eq!(
        r.header("x-amz-server-side-encryption-aws-kms-key-id"),
        Some("arn:aws:kms:us-east-1:000000000000:key/my-key")
    );
    assert_eq!(r.header("x-amz-server-side-encryption-bucket-key-enabled"), Some("true"));
    assert_ne!(r.header("etag"), Some("\"9dd4e461268c8034f5c8564e155c67a6\""));
    let h = c.head("/bkt/kms", &[]).await;
    assert_eq!(h.header("x-amz-server-side-encryption"), Some("aws:kms"));
    assert_eq!(
        h.header("x-amz-server-side-encryption-aws-kms-key-id"),
        Some("arn:aws:kms:us-east-1:000000000000:key/my-key")
    );

    for bad in [
        vec![("x-amz-server-side-encryption", "rot13")],
        vec![
            ("x-amz-server-side-encryption", "AES256"),
            ("x-amz-server-side-encryption-aws-kms-key-id", "k"),
        ],
        vec![("x-amz-server-side-encryption-aws-kms-key-id", "k")],
    ] {
        let r = c.put("/bkt/bad", &bad, "").await;
        assert_eq!((r.status, r.code()), (400, "InvalidArgument"), "{bad:?}");
    }

    // Default encryption applies to new objects only, and explicit headers win.
    let conf = "<ServerSideEncryptionConfiguration><Rule><ApplyServerSideEncryptionByDefault><SSEAlgorithm>aws:kms</SSEAlgorithm><KMSMasterKeyID>bucket-key</KMSMasterKeyID></ApplyServerSideEncryptionByDefault><BucketKeyEnabled>true</BucketKeyEnabled></Rule></ServerSideEncryptionConfiguration>";
    assert_eq!(c.put("/bkt?encryption", &[], conf).await.status, 200);
    let g = c.get("/bkt?encryption", &[]).await;
    assert!(
        g.body.contains("<SSEAlgorithm>aws:kms</SSEAlgorithm>") && g.body.contains("bucket-key"),
        "{}",
        g.body
    );
    let r = c.put("/bkt/new", &[], "x").await;
    assert_eq!(r.header("x-amz-server-side-encryption"), Some("aws:kms"));
    assert_eq!(
        r.header("x-amz-server-side-encryption-aws-kms-key-id"),
        Some("arn:aws:kms:us-east-1:000000000000:key/bucket-key")
    );
    assert_eq!(c.head("/bkt/k", &[]).await.header("x-amz-server-side-encryption"), Some("AES256"));
    let r = c.put("/bkt/explicit", &[("x-amz-server-side-encryption", "AES256")], "x").await;
    assert_eq!(r.header("x-amz-server-side-encryption"), Some("AES256"));
    let r = c.put("/bkt/c", &[("x-amz-copy-source", "/bkt/k")], "").await;
    assert_eq!(
        r.header("x-amz-server-side-encryption"),
        Some("aws:kms"),
        "copies take the bucket default"
    );
    // Multipart uploads pick it up at initiation.
    let r = c.call("POST", "/bkt/mp?uploads", &[], b"").await;
    assert_eq!(r.header("x-amz-server-side-encryption"), Some("aws:kms"));
    let bad = "<ServerSideEncryptionConfiguration><Rule><ApplyServerSideEncryptionByDefault><SSEAlgorithm>AES256</SSEAlgorithm><KMSMasterKeyID>k</KMSMasterKeyID></ApplyServerSideEncryptionByDefault></Rule></ServerSideEncryptionConfiguration>";
    assert_eq!(c.put("/bkt?encryption", &[], bad).await.code(), "InvalidArgument");

    assert_eq!(c.delete("/bkt?encryption", &[]).await.status, 204);
    assert_eq!(
        c.put("/bkt/after", &[], "x").await.header("x-amz-server-side-encryption"),
        Some("AES256")
    );
    let g = c.get("/bkt?encryption", &[]).await;
    assert!(g.body.contains("<SSEAlgorithm>AES256</SSEAlgorithm>"), "{}", g.body);
}

#[tokio::test]
async fn storage_class_and_redirect_are_echoed() {
    let (_server, c) = start().await;
    c.put("/bkt", &[], "").await;
    let r = c
        .put(
            "/bkt/k",
            &[
                ("x-amz-storage-class", "STANDARD_IA"),
                ("x-amz-website-redirect-location", "/other"),
            ],
            "x",
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.body);
    let h = c.head("/bkt/k", &[]).await;
    assert_eq!(h.header("x-amz-storage-class"), Some("STANDARD_IA"));
    assert_eq!(h.header("x-amz-website-redirect-location"), Some("/other"));
    assert_eq!(c.get("/bkt/k", &[]).await.header("x-amz-storage-class"), Some("STANDARD_IA"));
    let l = c.get("/bkt?list-type=2", &[]).await;
    assert!(l.body.contains("<StorageClass>STANDARD_IA</StorageClass>"), "{}", l.body);
    let v = c.get("/bkt?versions", &[]).await;
    assert!(v.body.contains("<StorageClass>STANDARD_IA</StorageClass>"), "{}", v.body);
    // STANDARD is not reported in headers.
    c.put("/bkt/std", &[], "x").await;
    assert_eq!(c.head("/bkt/std", &[]).await.header("x-amz-storage-class"), None);
    assert!(
        c.get("/bkt?list-type=2", &[])
            .await
            .body
            .contains("<StorageClass>STANDARD</StorageClass>")
    );

    for class in [
        "GLACIER",
        "DEEP_ARCHIVE",
        "GLACIER_IR",
        "INTELLIGENT_TIERING",
        "ONEZONE_IA",
        "REDUCED_REDUNDANCY",
    ] {
        assert_eq!(c.put("/bkt/c", &[("x-amz-storage-class", class)], "x").await.status, 200, "{class}");
        assert_eq!(c.head("/bkt/c", &[]).await.header("x-amz-storage-class"), Some(class));
    }
    let r = c.put("/bkt/c", &[("x-amz-storage-class", "COLD")], "x").await;
    assert_eq!((r.status, r.code()), (400, "InvalidStorageClass"));
    let r = c.put("/bkt/c", &[("x-amz-website-redirect-location", "nope")], "x").await;
    assert_eq!((r.status, r.code()), (400, "InvalidArgument"));

    // CopyObject: storage class is not inherited, the redirect is not copied, and a self-copy that
    // changes the storage class is legal.
    let r = c.put("/bkt/copy", &[("x-amz-copy-source", "/bkt/k")], "").await;
    assert_eq!(r.status, 200);
    let h = c.head("/bkt/copy", &[]).await;
    assert_eq!(
        (h.header("x-amz-storage-class"), h.header("x-amz-website-redirect-location")),
        (None, None)
    );
    let r = c.put("/bkt/k", &[("x-amz-copy-source", "/bkt/k")], "").await;
    assert_eq!(r.code(), "InvalidRequest");
    let r = c
        .put(
            "/bkt/k",
            &[("x-amz-copy-source", "/bkt/k"), ("x-amz-storage-class", "GLACIER_IR")],
            "",
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(c.head("/bkt/k", &[]).await.header("x-amz-storage-class"), Some("GLACIER_IR"));
}

#[tokio::test]
async fn get_object_attributes() {
    let (_server, c) = start().await;
    c.put("/bkt", &[], "").await;
    let r = c.put("/bkt/k", &[("x-amz-storage-class", "STANDARD_IA")], "hello").await;
    let etag = r.header("etag").unwrap().trim_matches('"').to_owned();
    let a = c
        .get("/bkt/k?attributes", &[("x-amz-object-attributes", "ETag,ObjectSize,StorageClass")])
        .await;
    assert_eq!(a.status, 200, "{}", a.body);
    assert_eq!(tag(&a.body, "ETag").unwrap().trim_matches('"'), etag);
    assert_eq!(tag(&a.body, "ObjectSize"), Some("5"));
    assert_eq!(tag(&a.body, "StorageClass"), Some("STANDARD_IA"));
    assert!(
        !a.body.contains("<Checksum>") && !a.body.contains("ObjectParts"),
        "only requested attributes: {}",
        a.body
    );
    assert!(a.header("last-modified").is_some());
    let a = c.get("/bkt/k?attributes", &[("x-amz-object-attributes", "Checksum")]).await;
    assert!(a.body.contains("<ChecksumCRC64NVME>"), "{}", a.body);

    let r = c.get("/bkt/k?attributes", &[("x-amz-object-attributes", "Bogus")]).await;
    assert_eq!((r.status, r.code()), (400, "InvalidArgument"));
    assert_eq!(c.get("/bkt/k?attributes", &[]).await.status, 400);
    assert_eq!(
        c.get("/bkt/missing?attributes", &[("x-amz-object-attributes", "ETag")])
            .await
            .code(),
        "NoSuchKey"
    );

    // Multipart: part list with sizes and per-part checksums, paginated.
    let r = c
        .call("POST", "/bkt/mp?uploads", &[("x-amz-checksum-algorithm", "CRC32")], b"")
        .await;
    let upload = tag(&r.body, "UploadId").unwrap().to_owned();
    let five = "a".repeat(5 * 1024 * 1024);
    let mut parts = String::new();
    for (n, body) in [(1, five.as_str()), (2, five.as_str()), (3, "tail")] {
        let r = c
            .put(
                &format!("/bkt/mp?partNumber={n}&uploadId={upload}"),
                &[("x-amz-checksum-algorithm", "CRC32")],
                body,
            )
            .await;
        assert_eq!(r.status, 200, "{}", r.body);
        parts += &format!(
            "<Part><PartNumber>{n}</PartNumber><ETag>{}</ETag></Part>",
            r.header("etag").unwrap()
        );
    }
    let r = c
        .call(
            "POST",
            &format!("/bkt/mp?uploadId={upload}"),
            &[],
            format!("<CompleteMultipartUpload>{parts}</CompleteMultipartUpload>").as_bytes(),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.body);
    let a = c
        .get("/bkt/mp?attributes", &[("x-amz-object-attributes", "ObjectParts,Checksum,ETag")])
        .await;
    assert_eq!(a.status, 200, "{}", a.body);
    assert_eq!(tag(&a.body, "PartsCount"), Some("3"), "{}", a.body);
    assert_eq!(tag(&a.body, "IsTruncated"), Some("false"));
    assert_eq!(a.body.matches("<Part>").count(), 3);
    assert_eq!(a.body.matches("<ChecksumCRC32>").count(), 4, "object + 3 parts: {}", a.body);
    assert!(tag(&a.body, "ETag").unwrap().trim_matches('"').ends_with("-3"));
    let a = c
        .get(
            "/bkt/mp?attributes",
            &[
                ("x-amz-object-attributes", "ObjectParts"),
                ("x-amz-max-parts", "2"),
                ("x-amz-part-number-marker", "0"),
            ],
        )
        .await;
    assert_eq!(tag(&a.body, "IsTruncated"), Some("true"));
    assert_eq!(tag(&a.body, "NextPartNumberMarker"), Some("2"));
    assert_eq!(a.body.matches("<Part>").count(), 2);
    assert!(
        a.body.contains("<PartNumber>1</PartNumber>") && a.body.contains("<PartNumber>2</PartNumber>"),
        "{}",
        a.body
    );
}

#[tokio::test]
async fn mfa_delete_and_versioning_details() {
    let (_server, c) = start().await;
    c.put("/bkt", &[], "").await;
    let conf = "<VersioningConfiguration><Status>Enabled</Status><MfaDelete>Enabled</MfaDelete></VersioningConfiguration>";
    assert_eq!(
        c.put("/bkt?versioning", &[], conf).await.code(),
        "AccessDenied",
        "needs the x-amz-mfa header"
    );
    assert_eq!(
        c.put("/bkt?versioning", &[("x-amz-mfa", "bogus")], conf).await.code(),
        "InvalidRequest"
    );
    let mfa = [("x-amz-mfa", "arn:aws:iam::123456789012:mfa/root 123456")];
    assert_eq!(c.put("/bkt?versioning", &mfa, conf).await.status, 200);
    let g = c.get("/bkt?versioning", &[]).await;
    assert!(
        g.body.contains("<MfaDelete>Enabled</MfaDelete>") && g.body.contains("<Status>Enabled</Status>"),
        "{}",
        g.body
    );

    let r = c.put("/bkt/k", &[], "x").await;
    let path = format!("/bkt/k?versionId={}", r.header("x-amz-version-id").unwrap());
    // Deleting without a version id is not MFA protected; permanently deleting a version is.
    assert_eq!(c.delete("/bkt/k", &[]).await.status, 204);
    let r = c.delete(&path, &[]).await;
    assert_eq!((r.status, r.code()), (403, "AccessDenied"));
    assert_eq!(c.delete(&path, &mfa).await.status, 204);
    let off = "<VersioningConfiguration><Status>Enabled</Status><MfaDelete>Disabled</MfaDelete></VersioningConfiguration>";
    assert_eq!(c.put("/bkt?versioning", &[], off).await.status, 403);
    assert_eq!(c.put("/bkt?versioning", &mfa, off).await.status, 200);
    assert!(c.get("/bkt?versioning", &[]).await.body.contains("<MfaDelete>Disabled</MfaDelete>"));
}

#[tokio::test]
async fn list_object_versions_details() {
    let (_server, c) = start().await;
    c.put("/bkt", &[], "").await;
    c.put(
        "/bkt?versioning",
        &[],
        "<VersioningConfiguration><Status>Enabled</Status></VersioningConfiguration>",
    )
    .await;
    for k in ["a", "b"] {
        c.put(&format!("/bkt/{k}"), &[], "1").await;
        c.put(&format!("/bkt/{k}"), &[], "2").await;
    }
    let v = c.get("/bkt?versions", &[]).await;
    assert!(
        v.body.contains("<KeyMarker></KeyMarker>") && v.body.contains("<VersionIdMarker></VersionIdMarker>"),
        "{}",
        v.body
    );
    let z = c.get("/bkt?versions&max-keys=0", &[]).await;
    assert_eq!(tag(&z.body, "IsTruncated"), Some("false"));
    assert!(!z.body.contains("<Version>"), "{}", z.body);
    let r = c.get("/bkt?versions&max-keys=-1", &[]).await;
    assert_eq!((r.status, r.code()), (400, "InvalidArgument"));
    let r = c.get("/bkt?versions&version-id-marker=0123456789abcdef0123456789abcdef", &[]).await;
    assert_eq!((r.status, r.code()), (400, "InvalidArgument"));
    // Made-up version ids are rejected up front, not reported as missing.
    let r = c.get("/bkt/a?versionId=nonsense", &[]).await;
    assert_eq!((r.status, r.code()), (400, "InvalidArgument"));
    let r = c.get("/bkt/a?versionId=0123456789abcdef0123456789abcdef", &[]).await;
    assert_eq!((r.status, r.code()), (404, "NoSuchVersion"));
    assert_eq!(c.delete("/bkt/a?versionId=nonsense", &[]).await.status, 400);
}

#[tokio::test]
async fn snapshot_restore_keeps_object_lock_and_encryption_state() {
    let (server, c) = start().await;
    c.lock_bucket("bkt").await;
    let cfg = "<ObjectLockConfiguration><ObjectLockEnabled>Enabled</ObjectLockEnabled><Rule><DefaultRetention><Mode>GOVERNANCE</Mode><Years>1</Years></DefaultRetention></Rule></ObjectLockConfiguration>";
    assert_eq!(c.put("/bkt?object-lock", &[], cfg).await.status, 200);
    let kms = "<ServerSideEncryptionConfiguration><Rule><ApplyServerSideEncryptionByDefault><SSEAlgorithm>aws:kms</SSEAlgorithm><KMSMasterKeyID>snap-key</KMSMasterKeyID></ApplyServerSideEncryptionByDefault></Rule></ServerSideEncryptionConfiguration>";
    assert_eq!(c.put("/bkt?encryption", &[], kms).await.status, 200);
    let mfa = [("x-amz-mfa", "serial 123456")];
    let conf = "<VersioningConfiguration><Status>Enabled</Status><MfaDelete>Enabled</MfaDelete></VersioningConfiguration>";
    assert_eq!(c.put("/bkt?versioning", &mfa, conf).await.status, 200);

    let r = c
        .put(
            "/bkt/obj",
            &[("x-amz-object-lock-legal-hold", "ON"), ("x-amz-storage-class", "STANDARD_IA")],
            "x",
        )
        .await;
    let vid = r.header("x-amz-version-id").unwrap().to_owned();
    let r = c
        .call(
            "POST",
            "/bkt/mp?uploads",
            &[("x-amz-server-side-encryption", "AES256"), ("x-amz-storage-class", "ONEZONE_IA")],
            b"",
        )
        .await;
    let upload = tag(&r.body, "UploadId").unwrap().to_owned();
    server.snapshot(Some("seeded")).unwrap();

    server.reset();
    assert_eq!(c.get("/bkt?object-lock", &[]).await.status, 404);
    server.restore("seeded").unwrap();

    // Bucket-level state.
    let g = c.get("/bkt?object-lock", &[]).await;
    assert!(
        g.body.contains("<Mode>GOVERNANCE</Mode>") && g.body.contains("<Years>1</Years>"),
        "{}",
        g.body
    );
    let g = c.get("/bkt?encryption", &[]).await;
    assert!(g.body.contains("aws:kms") && g.body.contains("snap-key"), "{}", g.body);
    assert!(c.get("/bkt?versioning", &[]).await.body.contains("<MfaDelete>Enabled</MfaDelete>"));
    // The restored defaults still apply to new objects, and the restored lock still enforces.
    let n = c.put("/bkt/new", &[], "y").await;
    assert_eq!(n.header("x-amz-server-side-encryption"), Some("aws:kms"));
    assert_eq!(c.head("/bkt/new", &[]).await.header("x-amz-object-lock-mode"), Some("GOVERNANCE"));
    // Object-level state.
    let path = format!("/bkt/obj?versionId={vid}");
    let h = c.head(&path, &[]).await;
    assert_eq!(h.header("x-amz-object-lock-legal-hold"), Some("ON"));
    assert_eq!(h.header("x-amz-storage-class"), Some("STANDARD_IA"));
    assert_eq!(
        c.delete(&path, &mfa).await.code(),
        "AccessDenied",
        "legal hold survives the restore"
    );
    assert_eq!(c.delete(&path, &[]).await.code(), "AccessDenied", "MFA Delete survives the restore");
    // Upload-level state.
    let part = c.put(&format!("/bkt/mp?partNumber=1&uploadId={upload}"), &[], "tail").await;
    let body = format!(
        "<CompleteMultipartUpload><Part><PartNumber>1</PartNumber><ETag>{}</ETag></Part></CompleteMultipartUpload>",
        part.header("etag").unwrap()
    );
    let done = c.call("POST", &format!("/bkt/mp?uploadId={upload}"), &[], body.as_bytes()).await;
    assert_eq!(done.status, 200, "{}", done.body);
    assert_eq!(c.head("/bkt/mp", &[]).await.header("x-amz-storage-class"), Some("ONEZONE_IA"));
}

#[tokio::test]
async fn lifecycle_respects_object_lock_and_headers_coexist() {
    let (server, c) = start().await;
    c.lock_bucket("bkt").await;
    let day = |n: u64| UNIX_EPOCH + Duration::from_secs(1_700_000_000 + n * 86_400);
    server.clock().freeze(Some(day(0))).unwrap();
    let lifecycle = "<LifecycleConfiguration>\
        <Rule><ID>old</ID><Status>Enabled</Status><Filter><Prefix></Prefix></Filter><NoncurrentVersionExpiration><NoncurrentDays>1</NoncurrentDays></NoncurrentVersionExpiration></Rule>\
        <Rule><ID>exp</ID><Status>Enabled</Status><Filter><Prefix>exp/</Prefix></Filter><Expiration><Days>30</Days></Expiration></Rule>\
        </LifecycleConfiguration>";
    assert_eq!(c.put("/bkt?lifecycle", &[], lifecycle).await.status, 200);

    // ACL, expiration, lock and SSE headers share one PUT and its HEAD.
    let r = c
        .put(
            "/bkt/exp/o",
            &[
                ("x-amz-acl", "private"),
                ("x-amz-server-side-encryption", "aws:kms"),
                ("x-amz-object-lock-mode", "GOVERNANCE"),
                ("x-amz-object-lock-retain-until-date", "2099-01-01T00:00:00Z"),
            ],
            "x",
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(
        r.header("x-amz-expiration").is_some_and(|e| e.contains("rule-id=\"exp\"")),
        "{:?}",
        r.headers
    );
    assert_eq!(r.header("x-amz-server-side-encryption"), Some("aws:kms"));
    let h = c.head("/bkt/exp/o", &[]).await;
    assert!(h.header("x-amz-expiration").is_some());
    assert_eq!(h.header("x-amz-object-lock-mode"), Some("GOVERNANCE"));
    assert_eq!(h.header("x-amz-server-side-encryption"), Some("aws:kms"));

    // A locked noncurrent version outlives its rule; an unlocked one does not.
    c.put(
        "/bkt/k",
        &[
            ("x-amz-object-lock-mode", "COMPLIANCE"),
            ("x-amz-object-lock-retain-until-date", "2023-12-01T00:00:00Z"),
        ],
        "1",
    )
    .await;
    c.put("/bkt/k", &[], "2").await;
    c.put("/bkt/j", &[], "1").await;
    c.put("/bkt/j", &[], "2").await;
    let versions_of = |body: &str, key: &str| body.matches(&format!("<Key>{key}</Key>")).count();
    server.clock().set(day(3)).unwrap();
    let v = c.get("/bkt?versions", &[]).await;
    assert_eq!((versions_of(&v.body, "k"), versions_of(&v.body, "j")), (2, 1), "{}", v.body);
    // 2023-12-01 is day 17 of this clock; after it the retention is over.
    server.clock().set(day(20)).unwrap();
    let v = c.get("/bkt?versions", &[]).await;
    assert_eq!((versions_of(&v.body, "k"), versions_of(&v.body, "j")), (1, 1), "{}", v.body);
}
