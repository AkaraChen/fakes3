//! Archive storage classes over real HTTP: InvalidObjectState, RestoreObject, Intelligent-Tiering
//! archive tiers and lifecycle transitions, all driven by the server's virtual clock. A bare-bones
//! HTTP/1.1 client keeps this free of SDK dependencies; its requests carry a signature the server
//! does not verify.

use std::time::{Duration, UNIX_EPOCH};

use fakes3::{Config, Server};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

mod common;

const MIN: i64 = 60 * 1_000_000_000;
const HOUR: i64 = 60 * MIN;
const DAY: i64 = 24 * HOUR;
/// 2030-01-01T10:30:00Z, a Tuesday.
const T0: u64 = 1_893_456_000 + 10 * 3600 + 1800;

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

    fn message(&self) -> String {
        tag(&self.body, "Message").unwrap_or("").replace("&apos;", "'")
    }
}

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
            "{method} {path} HTTP/1.1\r\nhost: {}\r\nconnection: close\r\ncontent-length: {}\r\nauthorization: {}\r\n",
            self.0,
            body.len(),
            common::UNVERIFIED_AUTH
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

    async fn post(&self, path: &str, body: &str) -> Resp {
        self.call("POST", path, &[], body.as_bytes()).await
    }

    async fn get(&self, path: &str, headers: &[(&str, &str)]) -> Resp {
        self.call("GET", path, headers, b"").await
    }

    async fn head(&self, path: &str) -> Resp {
        self.call("HEAD", path, &[], b"").await
    }

    async fn delete(&self, path: &str) -> Resp {
        self.call("DELETE", path, &[], b"").await
    }

    /// PUT an object with a storage class.
    async fn put_class(&self, key: &str, class: &str, body: &str) -> Resp {
        let r = self.put(&format!("/bkt/{key}"), &[("x-amz-storage-class", class)], body).await;
        assert_eq!(r.status, 200, "{}", r.body);
        r
    }

    async fn restore(&self, key: &str, body: &str) -> Resp {
        self.post(&format!("/bkt/{key}?restore"), body).await
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

/// A server with the clock frozen at [`T0`] and a bucket `bkt`.
async fn start() -> (Server, Client) {
    let server = Server::start(Config::default()).await.unwrap();
    server.clock().freeze(Some(UNIX_EPOCH + Duration::from_secs(T0))).unwrap();
    let c = Client(server.addr().to_string());
    assert_eq!(c.put("/bkt", &[], "").await.status, 200);
    (server, c)
}

fn days(d: u32, tier: &str) -> String {
    format!("<RestoreRequest><Days>{d}</Days><GlacierJobParameters><Tier>{tier}</Tier></GlacierJobParameters></RestoreRequest>")
}

#[tokio::test]
async fn archived_objects_refuse_reads_but_answer_head() {
    let (_s, c) = start().await;
    for class in ["GLACIER", "DEEP_ARCHIVE"] {
        let key = class.to_lowercase();
        c.put_class(&key, class, "payload").await;
        let path = format!("/bkt/{key}");
        for headers in [&[][..], &[("range", "bytes=0-2")][..]] {
            let r = c.get(&path, headers).await;
            assert_eq!((r.status, r.code()), (403, "InvalidObjectState"), "{class}: {}", r.body);
            assert_eq!(r.message(), "The action is not valid for the object's storage class");
        }
        let h = c.head(&path).await;
        assert_eq!(h.status, 200);
        assert_eq!(h.header("x-amz-storage-class"), Some(class));
        assert_eq!(h.header("content-length"), Some("7"));
        assert_eq!(h.header("x-amz-restore"), None, "never restored");
        // Metadata operations are not blocked.
        assert_eq!(c.get(&format!("{path}?tagging"), &[]).await.status, 200);
        let a = c
            .get(
                &format!("{path}?attributes"),
                &[("x-amz-object-attributes", "StorageClass,ObjectSize")],
            )
            .await;
        assert_eq!(a.status, 200, "{}", a.body);
        assert!(a.body.contains(&format!("<StorageClass>{class}</StorageClass>")), "{}", a.body);
    }
    // Glacier Instant Retrieval and the other classes are readable.
    for class in [
        "GLACIER_IR",
        "STANDARD_IA",
        "ONEZONE_IA",
        "INTELLIGENT_TIERING",
        "REDUCED_REDUNDANCY",
    ] {
        c.put_class("live", class, "payload").await;
        let r = c.get("/bkt/live", &[]).await;
        assert_eq!((r.status, r.body.as_str()), (200, "payload"), "{class}");
        assert_eq!(c.head("/bkt/live").await.header("x-amz-restore"), None);
    }
    // Restoring an object that is not archived is refused.
    let r = c.restore("live", &days(1, "Standard")).await;
    assert_eq!((r.status, r.code()), (403, "ObjectAlreadyInActiveTierError"), "{}", r.body);
    c.put("/bkt/std", &[], "x").await;
    assert_eq!(
        c.restore("std", &days(1, "Standard")).await.code(),
        "ObjectAlreadyInActiveTierError"
    );
    assert_eq!(c.restore("missing", &days(1, "Standard")).await.code(), "NoSuchKey");
}

#[tokio::test]
async fn restore_runs_on_the_server_clock_and_the_copy_expires_at_midnight_utc() {
    let (s, c) = start().await;
    c.put_class("k", "GLACIER", "payload").await;

    let r = c.restore("k", &days(3, "Standard")).await;
    assert_eq!(r.status, 202, "{}", r.body);
    assert_eq!(c.head("/bkt/k").await.header("x-amz-restore"), Some("ongoing-request=\"true\""));
    assert_eq!(c.get("/bkt/k", &[]).await.code(), "InvalidObjectState");
    let again = c.restore("k", &days(3, "Standard")).await;
    assert_eq!((again.status, again.code()), (409, "RestoreAlreadyInProgress"));

    // Standard takes 3 hours on Glacier Flexible Retrieval.
    s.clock().advance(3 * HOUR - MIN).unwrap();
    assert_eq!(c.get("/bkt/k", &[]).await.code(), "InvalidObjectState");
    s.clock().advance(MIN).unwrap();

    // Completed at 2030-01-01 13:30; +3 days rounds up to midnight: 2030-01-05.
    let h = c.head("/bkt/k").await;
    assert_eq!(
        h.header("x-amz-restore"),
        Some("ongoing-request=\"false\", expiry-date=\"Sat, 05 Jan 2030 00:00:00 GMT\"")
    );
    assert_eq!(h.header("x-amz-storage-class"), Some("GLACIER"), "the class stays");
    let g = c.get("/bkt/k", &[]).await;
    assert_eq!((g.status, g.body.as_str()), (200, "payload"));
    assert!(g.header("x-amz-restore").unwrap().starts_with("ongoing-request=\"false\""));
    assert_eq!(c.get("/bkt/k", &[("range", "bytes=1-3")]).await.body, "ayl");

    // Restoring again changes only the lifetime, counted from now: 10 days from Jan 1 13:30.
    let r = c.restore("k", &days(10, "Standard")).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(
        c.head("/bkt/k").await.header("x-amz-restore"),
        Some("ongoing-request=\"false\", expiry-date=\"Sat, 12 Jan 2030 00:00:00 GMT\"")
    );

    // One second before midnight on the 12th it is still there; at midnight it is gone.
    let to_midnight = 11 * DAY - 13 * HOUR - 30 * MIN;
    s.clock().advance(to_midnight - 1_000_000_000).unwrap();
    assert_eq!(c.get("/bkt/k", &[]).await.status, 200);
    s.clock().advance(1_000_000_000).unwrap();
    assert_eq!(c.get("/bkt/k", &[]).await.code(), "InvalidObjectState");
    assert_eq!(c.head("/bkt/k").await.header("x-amz-restore"), None);
    assert_eq!(c.restore("k", &days(1, "Standard")).await.status, 202, "a new request after expiry");
}

#[tokio::test]
async fn tiers_have_class_specific_completion_times() {
    let (s, c) = start().await;
    let ready = |key: &'static str| {
        let c = &c;
        async move { c.get(&format!("/bkt/{key}"), &[]).await.status == 200 }
    };
    c.put_class("g-exp", "GLACIER", "x").await;
    c.put_class("g-bulk", "GLACIER", "x").await;
    c.put_class("g-default", "GLACIER", "x").await;
    c.put_class("d-std", "DEEP_ARCHIVE", "x").await;
    c.put_class("d-bulk", "DEEP_ARCHIVE", "x").await;
    c.put_class("d-exp", "DEEP_ARCHIVE", "x").await;

    assert_eq!(c.restore("g-exp", &days(1, "Expedited")).await.status, 202);
    assert_eq!(c.restore("g-bulk", &days(1, "Bulk")).await.status, 202);
    // Tier at the top level, and Standard when unspecified.
    assert_eq!(
        c.restore("g-default", "<RestoreRequest><Days>1</Days></RestoreRequest>")
            .await
            .status,
        202
    );
    assert_eq!(c.restore("d-std", &days(1, "Standard")).await.status, 202);
    assert_eq!(
        c.restore("d-bulk", "<RestoreRequest><Days>1</Days><Tier>Bulk</Tier></RestoreRequest>")
            .await
            .status,
        202
    );
    let r = c.restore("d-exp", &days(1, "Expedited")).await;
    assert_eq!((r.status, r.code()), (400, "InvalidArgument"), "no Expedited for Deep Archive");

    s.clock().advance(MIN).unwrap();
    assert!(ready("g-exp").await && !ready("g-bulk").await && !ready("g-default").await);
    s.clock().advance(3 * HOUR - MIN).unwrap();
    assert!(ready("g-default").await && !ready("g-bulk").await && !ready("d-std").await);
    s.clock().advance(2 * HOUR).unwrap();
    assert!(ready("g-bulk").await && !ready("d-std").await);
    s.clock().advance(7 * HOUR).unwrap();
    assert!(ready("d-std").await && !ready("d-bulk").await);
    s.clock().advance(36 * HOUR).unwrap();
    assert!(ready("d-bulk").await);
}

#[tokio::test]
async fn restore_request_validation_and_tier_upgrade() {
    let (s, c) = start().await;
    c.put_class("k", "GLACIER", "x").await;
    let r = c
        .restore(
            "k",
            "<RestoreRequest><GlacierJobParameters><Tier>Standard</Tier></GlacierJobParameters></RestoreRequest>",
        )
        .await;
    assert_eq!((r.status, r.code()), (400, "MalformedXML"), "Days is required");
    assert_eq!(c.restore("k", &days(0, "Standard")).await.code(), "InvalidArgument");
    assert_eq!(c.restore("k", &days(1, "Fast")).await.status, 400);
    let select = "<RestoreRequest><Type>SELECT</Type><Tier>Standard</Tier></RestoreRequest>";
    let r = c.restore("k", select).await;
    assert_eq!((r.status, r.code()), (501, "NotImplemented"));
    assert_eq!(c.head("/bkt/k").await.header("x-amz-restore"), None, "nothing was started");

    // An ongoing Bulk restore can be sped up, but not slowed down or repeated.
    assert_eq!(c.restore("k", &days(2, "Bulk")).await.status, 202);
    assert_eq!(c.restore("k", &days(2, "Bulk")).await.code(), "RestoreAlreadyInProgress");
    assert_eq!(c.restore("k", &days(2, "Standard")).await.status, 202);
    assert_eq!(c.restore("k", &days(2, "Bulk")).await.code(), "RestoreAlreadyInProgress");
    s.clock().advance(3 * HOUR).unwrap();
    assert_eq!(c.get("/bkt/k", &[]).await.status, 200, "finished at the faster tier's time");
}

#[tokio::test]
async fn restore_targets_a_version() {
    let (_s, c) = start().await;
    c.put(
        "/bkt?versioning",
        &[],
        "<VersioningConfiguration><Status>Enabled</Status></VersioningConfiguration>",
    )
    .await;
    let v1 = c
        .put_class("k", "GLACIER", "one")
        .await
        .header("x-amz-version-id")
        .unwrap()
        .to_owned();
    let v2 = c
        .put_class("k", "GLACIER", "two")
        .await
        .header("x-amz-version-id")
        .unwrap()
        .to_owned();
    assert_eq!(
        c.post(&format!("/bkt/k?restore&versionId={v1}"), &days(1, "Expedited"))
            .await
            .status,
        202
    );
    assert!(c.head(&format!("/bkt/k?versionId={v1}")).await.header("x-amz-restore").is_some());
    assert_eq!(c.head(&format!("/bkt/k?versionId={v2}")).await.header("x-amz-restore"), None);
    let r = c.post("/bkt/k?restore&versionId=nope", &days(1, "Expedited")).await;
    assert_eq!((r.status, r.code()), (400, "InvalidArgument"));
}

#[tokio::test]
async fn copy_sources_must_be_restored_and_copies_are_standard() {
    let (s, c) = start().await;
    c.put_class("k", "GLACIER", "payload").await;
    let copy = |c: &Client, extra: Vec<(&'static str, &'static str)>| {
        let mut h = vec![("x-amz-copy-source", "/bkt/k")];
        h.extend(extra);
        let c = c.0.clone();
        async move { Client(c).put("/bkt/dst", &h, "").await }
    };
    let r = copy(&c, vec![]).await;
    assert_eq!((r.status, r.code()), (403, "InvalidObjectState"), "{}", r.body);
    assert_eq!(r.message(), "Operation is not valid for the source object's storage class");
    assert_eq!(c.head("/bkt/dst").await.status, 404, "nothing was written");

    // UploadPartCopy is a read too.
    let up = c.post("/bkt/mp?uploads", "").await;
    let id = tag(&up.body, "UploadId").unwrap().to_owned();
    let r = c
        .put(
            &format!("/bkt/mp?partNumber=1&uploadId={id}"),
            &[("x-amz-copy-source", "/bkt/k")],
            "",
        )
        .await;
    assert_eq!((r.status, r.code()), (403, "InvalidObjectState"));

    c.restore("k", &days(1, "Expedited")).await;
    s.clock().advance(MIN).unwrap();
    assert_eq!(copy(&c, vec![]).await.status, 200);
    let h = c.head("/bkt/dst").await;
    assert_eq!(h.header("x-amz-storage-class"), None, "the copy is STANDARD");
    assert_eq!(h.header("x-amz-restore"), None);
    assert_eq!(c.get("/bkt/dst", &[]).await.body, "payload");
    // Copying a restored object into an archive class archives the copy.
    assert_eq!(copy(&c, vec![("x-amz-storage-class", "DEEP_ARCHIVE")]).await.status, 200);
    assert_eq!(c.get("/bkt/dst", &[]).await.code(), "InvalidObjectState");
    // Copying an object onto itself with a new class is how SDK users make a restore permanent.
    let r = c
        .put(
            "/bkt/k",
            &[("x-amz-copy-source", "/bkt/k"), ("x-amz-storage-class", "STANDARD")],
            "",
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(c.get("/bkt/k", &[]).await.body, "payload");
    assert_eq!(c.head("/bkt/k").await.header("x-amz-storage-class"), None);
}

#[tokio::test]
async fn list_objects_v2_reports_restore_status_when_asked() {
    let (s, c) = start().await;
    c.put_class("a-never", "GLACIER", "x").await;
    c.put_class("b-ongoing", "GLACIER", "x").await;
    c.put_class("c-done", "DEEP_ARCHIVE", "x").await;
    c.put("/bkt/d-plain", &[], "x").await;
    c.restore("c-done", &days(1, "Standard")).await;
    s.clock().advance(12 * HOUR).unwrap();
    c.restore("b-ongoing", &days(1, "Bulk")).await;

    let plain = c.get("/bkt?list-type=2", &[]).await;
    assert!(!plain.body.contains("RestoreStatus"), "{}", plain.body);
    let r = c
        .get("/bkt?list-type=2", &[("x-amz-optional-object-attributes", "RestoreStatus")])
        .await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.body.matches("<RestoreStatus>").count(), 2, "{}", r.body);
    assert!(
        r.body
            .contains("<RestoreStatus><IsRestoreInProgress>true</IsRestoreInProgress></RestoreStatus>"),
        "{}",
        r.body
    );
    assert!(
        r.body
            .contains("<IsRestoreInProgress>false</IsRestoreInProgress><RestoreExpiryDate>2030-01-03T00:00:00.000Z</RestoreExpiryDate>"),
        "{}",
        r.body
    );
}

fn lifecycle(rules: &str) -> String {
    format!("<LifecycleConfiguration>{rules}</LifecycleConfiguration>")
}

fn rule(filter: &str, actions: &str) -> String {
    format!("<Rule><ID>r</ID><Filter>{filter}</Filter><Status>Enabled</Status>{actions}</Rule>")
}

fn transition(days: u32, class: &str) -> String {
    format!("<Transition><Days>{days}</Days><StorageClass>{class}</StorageClass></Transition>")
}

async fn put_lifecycle(c: &Client, xml: &str, min_size: Option<&str>) -> Resp {
    let h: Vec<(&str, &str)> = min_size
        .map(|m| ("x-amz-transition-default-minimum-object-size", m))
        .into_iter()
        .collect();
    c.put("/bkt?lifecycle", &h, xml).await
}

fn class(r: &Resp) -> &str {
    r.header("x-amz-storage-class").unwrap_or("STANDARD")
}

#[tokio::test]
async fn lifecycle_transitions_follow_the_waterfall_and_leave_data_alone() {
    let (s, c) = start().await;
    let big = "x".repeat(200 * 1024);
    let put = c.put("/bkt/big", &[], &big).await;
    let etag = put.header("etag").unwrap().to_owned();
    let before = c.head("/bkt/big").await;
    let xml = lifecycle(&rule(
        "",
        &format!("{}{}", transition(30, "STANDARD_IA"), transition(90, "DEEP_ARCHIVE")),
    ));
    assert_eq!(put_lifecycle(&c, &xml, None).await.status, 200);

    // Transitions are due at midnight UTC after creation + Days.
    s.clock().advance(30 * DAY).unwrap();
    assert_eq!(class(&c.head("/bkt/big").await), "STANDARD", "Jan 31 10:30 is before Feb 1 00:00");
    s.clock().advance(DAY).unwrap();
    let h = c.head("/bkt/big").await;
    assert_eq!(class(&h), "STANDARD_IA");
    assert_eq!(c.get("/bkt/big", &[]).await.body.len(), big.len(), "IA stays readable");
    assert_eq!(h.header("etag"), Some(etag.as_str()));
    assert_eq!(h.header("last-modified"), before.header("last-modified"));

    // A long jump lands on the deepest class that is due.
    s.clock().advance(100 * DAY).unwrap();
    let r = c.get("/bkt/big", &[]).await;
    assert_eq!((r.status, r.code()), (403, "InvalidObjectState"));
    assert_eq!(class(&c.head("/bkt/big").await), "DEEP_ARCHIVE");

    // Archived objects never move back up the waterfall, whatever the rules say.
    let xml = lifecycle(&rule("", &transition(1, "GLACIER_IR")));
    put_lifecycle(&c, &xml, None).await;
    s.clock().advance(2 * DAY).unwrap();
    assert_eq!(class(&c.head("/bkt/big").await), "DEEP_ARCHIVE");
    // Glacier -> Deep Archive is allowed.
    c.put_class("g", "GLACIER", &big).await;
    put_lifecycle(&c, &lifecycle(&rule("", &transition(1, "DEEP_ARCHIVE"))), None).await;
    s.clock().advance(3 * DAY).unwrap();
    assert_eq!(class(&c.head("/bkt/g").await), "DEEP_ARCHIVE");
}

#[tokio::test]
async fn lifecycle_transitions_respect_the_minimum_object_size() {
    let (s, c) = start().await;
    let big = "x".repeat(128 * 1024);
    c.put("/bkt/small", &[], &big[..big.len() - 1]).await;
    c.put("/bkt/exact", &[], &big).await;

    let xml = lifecycle(&rule("", &format!("{}{}", transition(1, "STANDARD_IA"), transition(2, "GLACIER"))));
    assert_eq!(put_lifecycle(&c, &xml, None).await.status, 200);
    s.clock().advance(5 * DAY).unwrap();
    assert_eq!(
        class(&c.head("/bkt/small").await),
        "STANDARD",
        "below 128 KB nothing happens by default"
    );
    assert_eq!(class(&c.head("/bkt/exact").await), "GLACIER");

    // The legacy default still sends small objects to the Glacier classes, but not to IA.
    let (s, c) = start().await;
    c.put("/bkt/small", &[], "tiny").await;
    let legacy = Some("varies_by_storage_class");
    assert_eq!(
        put_lifecycle(&c, &lifecycle(&rule("", &transition(1, "STANDARD_IA"))), legacy)
            .await
            .status,
        200
    );
    s.clock().advance(2 * DAY).unwrap();
    assert_eq!(class(&c.head("/bkt/small").await), "STANDARD");
    put_lifecycle(&c, &lifecycle(&rule("", &transition(1, "GLACIER"))), legacy).await;
    s.clock().advance(2 * DAY).unwrap();
    assert_eq!(class(&c.head("/bkt/small").await), "GLACIER");

    // A size filter replaces the default minimum.
    let (s, c) = start().await;
    c.put("/bkt/small", &[], "tiny").await;
    let filter = "<ObjectSizeLessThan>1000</ObjectSizeLessThan>";
    put_lifecycle(&c, &lifecycle(&rule(filter, &transition(1, "STANDARD_IA"))), None).await;
    s.clock().advance(2 * DAY).unwrap();
    assert_eq!(class(&c.head("/bkt/small").await), "STANDARD_IA");

    let bad = put_lifecycle(&c, &lifecycle(&rule("", &transition(1, "GLACIER"))), Some("whatever")).await;
    assert_eq!((bad.status, bad.code()), (400, "InvalidArgument"));
}

#[tokio::test]
async fn lifecycle_date_transitions_and_validation() {
    let (s, c) = start().await;
    let big = "x".repeat(130 * 1024);
    c.put("/bkt/k", &[], &big).await;
    let date = "<Transition><Date>2030-02-01T00:00:00.000Z</Date><StorageClass>GLACIER</StorageClass></Transition>";
    assert_eq!(put_lifecycle(&c, &lifecycle(&rule("", date)), None).await.status, 200);
    s.clock().advance(30 * DAY).unwrap();
    assert_eq!(class(&c.head("/bkt/k").await), "STANDARD");
    s.clock().advance(DAY).unwrap();
    assert_eq!(class(&c.head("/bkt/k").await), "GLACIER");

    let bad_date = "<Transition><Date>2030-02-01T10:00:00.000Z</Date><StorageClass>GLACIER</StorageClass></Transition>";
    let r = put_lifecycle(&c, &lifecycle(&rule("", bad_date)), None).await;
    assert_eq!((r.status, r.code()), (400, "InvalidArgument"), "{}", r.body);
    for class in ["STANDARD", "REDUCED_REDUNDANCY", "NOPE"] {
        let r = put_lifecycle(&c, &lifecycle(&rule("", &transition(1, class))), None).await;
        assert_eq!((r.status, r.code()), (400, "MalformedXML"), "{class}");
    }
    // A disabled rule is validated but never fires.
    let disabled = lifecycle(&rule("", &transition(1, "DEEP_ARCHIVE")).replace("Enabled", "Disabled"));
    c.put("/bkt/off", &[], &big).await;
    put_lifecycle(&c, &disabled, None).await;
    s.clock().advance(5 * DAY).unwrap();
    assert_eq!(class(&c.head("/bkt/off").await), "STANDARD");
}

#[tokio::test]
async fn lifecycle_transitions_noncurrent_versions() {
    let (s, c) = start().await;
    c.put(
        "/bkt?versioning",
        &[],
        "<VersioningConfiguration><Status>Enabled</Status></VersioningConfiguration>",
    )
    .await;
    let big = "x".repeat(130 * 1024);
    let mut ids = Vec::new();
    for _ in 0..4 {
        ids.push(c.put("/bkt/k", &[], &big).await.header("x-amz-version-id").unwrap().to_owned());
        s.clock().advance(HOUR).unwrap();
    }
    let nc = "<NoncurrentVersionTransition><NoncurrentDays>1</NoncurrentDays><NewerNoncurrentVersions>1</NewerNoncurrentVersions><StorageClass>GLACIER</StorageClass></NoncurrentVersionTransition>";
    assert_eq!(put_lifecycle(&c, &lifecycle(&rule("", nc)), None).await.status, 200);
    s.clock().advance(2 * DAY).unwrap();
    let class_of = |id: String| {
        let c = &c;
        async move { class(&c.head(&format!("/bkt/k?versionId={id}")).await).to_owned() }
    };
    // v3 is current; v2 is the newest noncurrent version and stays; v1 and v0 move.
    assert_eq!(class_of(ids[3].clone()).await, "STANDARD");
    assert_eq!(class_of(ids[2].clone()).await, "STANDARD");
    assert_eq!(class_of(ids[1].clone()).await, "GLACIER");
    assert_eq!(class_of(ids[0].clone()).await, "GLACIER");
    assert_eq!(c.get("/bkt/k", &[]).await.status, 200);
    assert_eq!(
        c.get(&format!("/bkt/k?versionId={}", ids[0]), &[]).await.code(),
        "InvalidObjectState"
    );
}

#[tokio::test]
async fn object_lock_does_not_block_transitions() {
    let (s, c) = start().await;
    assert_eq!(
        c.put("/lock", &[("x-amz-bucket-object-lock-enabled", "true")], "").await.status,
        200
    );
    let big = "x".repeat(130 * 1024);
    let r = c
        .put(
            "/lock/k",
            &[
                ("x-amz-object-lock-mode", "COMPLIANCE"),
                ("x-amz-object-lock-retain-until-date", "2031-01-01T00:00:00Z"),
            ],
            &big,
        )
        .await;
    let vid = r.header("x-amz-version-id").unwrap().to_owned();
    let xml = lifecycle(&rule(
        "",
        &format!("{}{}", transition(1, "GLACIER"), "<Expiration><Days>5</Days></Expiration>"),
    ));
    assert_eq!(c.put("/lock?lifecycle", &[], &xml).await.status, 200);
    s.clock().advance(10 * DAY).unwrap();
    let h = c.head("/lock/k").await;
    // Expiration added a delete marker; the locked version underneath moved and kept its lock.
    assert_eq!(h.status, 404);
    let h = c.head(&format!("/lock/k?versionId={vid}")).await;
    assert_eq!(class(&h), "GLACIER");
    assert_eq!(h.header("x-amz-object-lock-mode"), Some("COMPLIANCE"));
    let d = c.delete(&format!("/lock/k?versionId={vid}")).await;
    assert_eq!((d.status, d.code()), (403, "AccessDenied"));
}

fn it_config(id: &str, tiers: &[(&str, u32)]) -> String {
    let tiers: String = tiers
        .iter()
        .map(|(t, d)| format!("<Tiering><Days>{d}</Days><AccessTier>{t}</AccessTier></Tiering>"))
        .collect();
    format!("<IntelligentTieringConfiguration><Id>{id}</Id><Status>Enabled</Status>{tiers}</IntelligentTieringConfiguration>")
}

#[tokio::test]
async fn intelligent_tiering_archive_tiers() {
    let (s, c) = start().await;
    c.put_class("k", "INTELLIGENT_TIERING", "payload").await;
    s.clock().advance(400 * DAY).unwrap();
    assert_eq!(c.get("/bkt/k", &[]).await.status, 200, "without a configuration nothing archives");

    let cfg = it_config("cfg", &[("ARCHIVE_ACCESS", 90), ("DEEP_ARCHIVE_ACCESS", 180)]);
    assert_eq!(c.put("/bkt?intelligent-tiering&id=cfg", &[], &cfg).await.status, 200);
    let g = c.get("/bkt?intelligent-tiering&id=cfg", &[]).await;
    assert_eq!(g.status, 200, "{}", g.body);
    assert!(g.body.contains("<AccessTier>ARCHIVE_ACCESS</AccessTier>"));
    assert!(c.get("/bkt?intelligent-tiering", &[]).await.body.contains("<Id>cfg</Id>"));

    // The object is 400 days old: Deep Archive Access. A plain GET fails.
    let r = c.get("/bkt/k", &[]).await;
    assert_eq!((r.status, r.code()), (403, "InvalidObjectState"));
    assert_eq!(r.message(), "The action is not valid for the object's access tier");
    let h = c.head("/bkt/k").await;
    assert_eq!(h.header("x-amz-archive-status"), Some("DEEP_ARCHIVE_ACCESS"));
    assert_eq!(h.header("x-amz-storage-class"), Some("INTELLIGENT_TIERING"));

    // Restores take no Days; Deep Archive Access is 12 hours at Standard, and Expedited is refused.
    assert_eq!(c.restore("k", &days(1, "Expedited")).await.code(), "InvalidArgument");
    assert_eq!(c.restore("k", "<RestoreRequest/>").await.status, 202);
    assert_eq!(c.head("/bkt/k").await.header("x-amz-restore"), Some("ongoing-request=\"true\""));
    s.clock().advance(12 * HOUR).unwrap();
    // Back in Frequent Access: readable, and no restore bookkeeping is reported.
    let g = c.get("/bkt/k", &[]).await;
    assert_eq!((g.status, g.body.as_str()), (200, "payload"));
    let h = c.head("/bkt/k").await;
    assert_eq!((h.header("x-amz-restore"), h.header("x-amz-archive-status")), (None, None));
    assert_eq!(c.restore("k", "<RestoreRequest/>").await.code(), "ObjectAlreadyInActiveTierError");

    // The idle count starts again: Archive Access after 90 days, Deep Archive Access after 180.
    s.clock().advance(89 * DAY).unwrap();
    assert_eq!(c.get("/bkt/k", &[]).await.status, 200);
    s.clock().advance(DAY).unwrap();
    assert_eq!(c.head("/bkt/k").await.header("x-amz-archive-status"), Some("ARCHIVE_ACCESS"));
    // Archive Access: Expedited in 1 minute.
    assert_eq!(c.restore("k", &days(1, "Expedited")).await.status, 202);
    s.clock().advance(MIN).unwrap();
    assert_eq!(c.get("/bkt/k", &[]).await.status, 200);
}

#[tokio::test]
async fn intelligent_tiering_configuration_api_and_filters() {
    let (s, c) = start().await;
    let ok = it_config("a", &[("ARCHIVE_ACCESS", 90)]);
    assert_eq!(c.put("/bkt?intelligent-tiering&id=a", &[], &ok).await.status, 200);
    assert_eq!(
        c.put("/bkt?intelligent-tiering&id=other", &[], &ok).await.code(),
        "InvalidArgument",
        "id mismatch"
    );
    for (id, tiers) in [
        ("short", vec![("ARCHIVE_ACCESS", 30)]),
        ("deep", vec![("DEEP_ARCHIVE_ACCESS", 90)]),
        ("dup", vec![("ARCHIVE_ACCESS", 90), ("ARCHIVE_ACCESS", 100)]),
        ("long", vec![("ARCHIVE_ACCESS", 731)]),
    ] {
        let r = c
            .put(&format!("/bkt?intelligent-tiering&id={id}"), &[], &it_config(id, &tiers))
            .await;
        assert_eq!((r.status, r.code()), (400, "InvalidArgument"), "{id}: {}", r.body);
    }
    assert_eq!(c.get("/bkt?intelligent-tiering&id=zzz", &[]).await.status, 404);
    assert_eq!(c.delete("/bkt?intelligent-tiering&id=zzz").await.code(), "NoSuchConfiguration");
    assert_eq!(c.delete("/bkt?intelligent-tiering&id=a").await.status, 204);
    assert_eq!(c.get("/bkt?intelligent-tiering&id=a", &[]).await.status, 404);

    // A prefix filter limits which objects archive.
    let filtered = it_config("p", &[("ARCHIVE_ACCESS", 90)]).replace("<Status>", "<Filter><Prefix>cold/</Prefix></Filter><Status>");
    assert_eq!(c.put("/bkt?intelligent-tiering&id=p", &[], &filtered).await.status, 200);
    c.put_class("cold/a", "INTELLIGENT_TIERING", "x").await;
    c.put_class("warm/a", "INTELLIGENT_TIERING", "x").await;
    s.clock().advance(91 * DAY).unwrap();
    assert_eq!(c.get("/bkt/cold/a", &[]).await.code(), "InvalidObjectState");
    assert_eq!(c.get("/bkt/warm/a", &[]).await.status, 200);
}

#[tokio::test]
async fn lifecycle_can_transition_into_intelligent_tiering_archive_tiers() {
    let (s, c) = start().await;
    let cfg = it_config("cfg", &[("ARCHIVE_ACCESS", 90)]);
    c.put("/bkt?intelligent-tiering&id=cfg", &[], &cfg).await;
    c.put("/bkt/k", &[], &"x".repeat(130 * 1024)).await;
    put_lifecycle(&c, &lifecycle(&rule("", &transition(10, "INTELLIGENT_TIERING"))), None).await;
    s.clock().advance(11 * DAY).unwrap();
    assert_eq!(class(&c.head("/bkt/k").await), "INTELLIGENT_TIERING");
    // 90 idle days are counted from the transition (Jan 12 00:00), not from the upload.
    s.clock().advance(79 * DAY).unwrap();
    assert_eq!(c.get("/bkt/k", &[]).await.status, 200);
    s.clock().advance(11 * DAY).unwrap();
    assert_eq!(c.get("/bkt/k", &[]).await.code(), "InvalidObjectState");
}

#[tokio::test]
async fn snapshots_keep_restore_state_and_transitions_and_memory_is_untouched() {
    let (s, c) = start().await;
    let big = "x".repeat(130 * 1024);
    c.put_class("k", "GLACIER", &big).await;
    c.put("/bkt/t", &[], &big).await;
    put_lifecycle(&c, &lifecycle(&rule("", &transition(1, "GLACIER"))), None).await;
    let cfg = it_config("cfg", &[("ARCHIVE_ACCESS", 90)]);
    c.put("/bkt?intelligent-tiering&id=cfg", &[], &cfg).await;
    c.restore("k", &days(2, "Bulk")).await;
    let used = s.memory().used;

    s.snapshot(Some("snap")).unwrap();
    s.clock().advance(2 * DAY).unwrap();
    assert_eq!(class(&c.head("/bkt/t").await), "GLACIER", "transitioned after the snapshot");
    assert!(
        c.head("/bkt/k").await.header("x-amz-restore").unwrap().contains("expiry-date"),
        "5 h Bulk restore done"
    );
    assert_eq!(s.memory().used, used, "transitions and restores do not touch the accounting");

    // Roll everything back and move the clock to just before the Bulk restore finishes.
    s.clock().advance(-2 * DAY + 4 * HOUR).unwrap();
    s.restore("snap").unwrap();
    assert_eq!(s.memory().used, used);
    assert_eq!(class(&c.head("/bkt/t").await), "STANDARD", "the transition is undone");
    assert_eq!(c.head("/bkt/k").await.header("x-amz-restore"), Some("ongoing-request=\"true\""));
    assert!(
        c.get("/bkt?intelligent-tiering&id=cfg", &[]).await.status == 200,
        "IT configs are restored"
    );
    s.clock().advance(HOUR).unwrap();
    assert_eq!(c.get("/bkt/k", &[]).await.status, 200, "the restored request completes on schedule");
    // The lifecycle rules came back with the snapshot too.
    s.clock().advance(2 * DAY).unwrap();
    assert_eq!(class(&c.head("/bkt/t").await), "GLACIER");
}
