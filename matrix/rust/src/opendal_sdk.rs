use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use opendal::{EntryMode, ErrorKind, Operator};

use crate::check;
use crate::common::*;

pub const SDK: &str = "opendal";

pub struct Ctx {
    pub env: Env,
    pub op: Operator,
    pub b: String,
    pub admin: aws_sdk_s3::Client,
}
type C = Arc<Ctx>;

fn operator(env: &Env, bucket: &str, secret: &str, v1: bool) -> Result<Operator> {
    let mut b = opendal::services::S3::default()
        .root("/")
        .bucket(bucket)
        .endpoint(&env.endpoint)
        .region(&env.region)
        .access_key_id(&env.ak)
        .secret_access_key(secret)
        .disable_config_load()
        .disable_ec2_metadata();
    if v1 {
        b = b.disable_list_objects_v2();
    }
    Ok(Operator::new(b)?)
}

fn oerr(e: opendal::Error) -> anyhow::Error {
    anyhow!("opendal::Error::{:?}: {}", e.kind(), e)
}

fn expect_kind<T>(r: opendal::Result<T>, kinds: &[ErrorKind], what: &str) -> Result<()> {
    match r {
        Ok(_) => Err(anyhow!("AssertionError: expected {what}, got success")),
        Err(e) if kinds.contains(&e.kind()) => Ok(()),
        Err(e) => Err(anyhow!("AssertionError: expected {what}, got {}", oerr(e))),
    }
}

async fn put(c: &C, key: &str, data: &[u8]) -> Result<()> {
    c.op.write(key, data.to_vec()).await.map_err(oerr)?;
    Ok(())
}

async fn get(c: &C, key: &str) -> Result<Vec<u8>> {
    Ok(c.op.read(key).await.map_err(oerr)?.to_vec())
}

const NO_BUCKET_API: &str = "opendal has no bucket management API";

async fn put_get_small(c: C) -> Result<()> {
    let data = b"hello world";
    let m = c.op.write("small.txt", data.to_vec()).await.map_err(oerr)?;
    if let Some(e) = m.etag() {
        check!(unquote(e) == md5_hex(data), "write ETag {e} != md5");
    }
    check!(get(&c, "small.txt").await? == data, "body mismatch");
    let st = c.op.stat("small.txt").await.map_err(oerr)?;
    let etag = unquote(st.etag().unwrap_or_default());
    check!(etag == md5_hex(data), "ETag {etag} != md5");
    Ok(())
}

async fn put_get_empty(c: C) -> Result<()> {
    put(&c, "empty", b"").await?;
    check!(get(&c, "empty").await?.is_empty(), "body not empty");
    let st = c.op.stat("empty").await.map_err(oerr)?;
    check!(st.content_length() == 0, "Content-Length {}", st.content_length());
    Ok(())
}

async fn put_get_large(c: C) -> Result<()> {
    let data = rand_bytes(8 << 20);
    put(&c, "large.bin", &data).await?;
    check!(get(&c, "large.bin").await? == data, "8MiB body mismatch");
    Ok(())
}

async fn special_keys(c: C) -> Result<()> {
    let mut tested = vec![];
    for (i, k) in SPECIAL_KEYS.iter().enumerate() {
        if k.contains("//") {
            continue; // opendal normalizes "a//b" to "a/b"
        }
        let data = format!("special-{i}");
        put(&c, k, data.as_bytes()).await?;
        check!(get(&c, k).await? == data.as_bytes(), "body mismatch for {k:?}");
        tested.push(*k);
    }
    let listed: Vec<String> =
        c.op.list_with("/").recursive(true).await.map_err(oerr)?.iter().map(|e| e.path().to_string()).collect();
    for k in &tested {
        check!(listed.iter().any(|x| x == k), "key {k:?} not listed");
    }
    Err(skip(format!(
        "opendal normalizes paths and cannot address key \"a//b\"; other {} keys passed",
        tested.len()
    )))
}

fn want_meta() -> HashMap<String, String> {
    [("color", "Blue"), ("x-y", "z")].iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
}

async fn head_object_metadata(c: C) -> Result<()> {
    c.op.write_with("meta.txt", b"metadata".to_vec())
        .content_type("text/x-matrix")
        .cache_control("max-age=3600")
        .content_disposition("attachment; filename=\"m.txt\"")
        .content_encoding("gzip")
        .user_metadata(want_meta())
        .await
        .map_err(oerr)?;
    let m = c.op.stat("meta.txt").await.map_err(oerr)?;
    check!(m.content_type() == Some("text/x-matrix"), "Content-Type {:?}", m.content_type());
    check!(m.cache_control() == Some("max-age=3600"), "Cache-Control {:?}", m.cache_control());
    check!(
        m.content_disposition() == Some("attachment; filename=\"m.txt\""),
        "Content-Disposition {:?}",
        m.content_disposition()
    );
    check!(m.content_encoding() == Some("gzip"), "Content-Encoding {:?}", m.content_encoding());
    let um: Option<HashMap<String, String>> =
        m.user_metadata().map(|u| u.into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect());
    check!(um.as_ref() == Some(&want_meta()), "metadata {:?}", um);
    Ok(())
}

async fn get_range(c: C) -> Result<()> {
    // opendal does not expose Content-Range; only the returned bytes are verified
    let data = range_body();
    put(&c, "range.bin", &data).await?;
    for (label, lo, hi) in [("bytes=10-19", 10usize, 19usize), ("bytes=-5", 95, 99), ("bytes=95-", 95, 99)] {
        let range: opendal::BytesRange = label.parse().map_err(oerr)?;
        let got = c.op.read_with("range.bin").range(range).await.map_err(oerr)?.to_vec();
        check!(got == data[lo..=hi], "{label}: body mismatch ({} bytes)", got.len());
    }
    Ok(())
}

async fn get_range_invalid(c: C) -> Result<()> {
    put(&c, "range-inv.bin", &range_body()).await?;
    let r = c.op.read_with("range-inv.bin").range(200..301).await;
    expect_kind(r, &[ErrorKind::RangeNotSatisfied], "416 RangeNotSatisfied")
}

async fn get_missing_key(c: C) -> Result<()> {
    expect_kind(c.op.read("does/not/exist").await, &[ErrorKind::NotFound], "NotFound")
}

async fn get_if_none_match(c: C) -> Result<()> {
    put(&c, "inm.txt", b"if-none-match").await?;
    let etag = c.op.stat("inm.txt").await.map_err(oerr)?.etag().unwrap_or_default().to_string();
    let r = c.op.read_with("inm.txt").if_none_match(&etag).await;
    match r {
        Err(e) if e.kind() == ErrorKind::ConditionNotMatch && e.to_string().contains("304") => Ok(()),
        Err(e) => Err(anyhow!("AssertionError: expected 304, got {}", oerr(e))),
        Ok(_) => Err(anyhow!("AssertionError: expected 304, got success")),
    }
}

async fn get_if_match_fail(c: C) -> Result<()> {
    put(&c, "im.txt", b"if-match").await?;
    let r = c.op.read_with("im.txt").if_match("\"wrong\"").await;
    match r {
        Err(e) if e.kind() == ErrorKind::ConditionNotMatch && e.to_string().contains("412") => Ok(()),
        Err(e) => Err(anyhow!("AssertionError: expected 412, got {}", oerr(e))),
        Ok(_) => Err(anyhow!("AssertionError: expected 412, got success")),
    }
}

async fn put_if_none_match(c: C) -> Result<()> {
    put(&c, "pinm.txt", b"first").await?;
    let r = c.op.write_with("pinm.txt", b"second".to_vec()).if_not_exists(true).await;
    expect_kind(r, &[ErrorKind::ConditionNotMatch, ErrorKind::AlreadyExists], "412")?;
    let k = format!("pinm-new-{}", rand_hex8());
    c.op.write_with(&k, b"new".to_vec()).if_not_exists(true).await.map_err(oerr)?;
    check!(get(&c, &k).await? == b"new", "new key body mismatch");
    check!(get(&c, "pinm.txt").await? == b"first", "existing key overwritten");
    Ok(())
}

async fn delete_object(c: C) -> Result<()> {
    put(&c, "del.txt", b"x").await?;
    c.op.delete("del.txt").await.map_err(oerr)?;
    expect_kind(c.op.stat("del.txt").await, &[ErrorKind::NotFound], "NotFound")
}

async fn delete_missing_object(c: C) -> Result<()> {
    c.op.delete(&format!("never-existed-{}", rand_hex8())).await.map_err(oerr)?;
    Ok(())
}

async fn delete_objects(c: C) -> Result<()> {
    let keys = ["bulk/1", "bulk/2", "bulk/3"];
    for k in keys {
        put(&c, k, k.as_bytes()).await?;
    }
    let owned: Vec<String> = keys.iter().map(|k| k.to_string()).collect();
    c.op.delete_iter(owned).await.map_err(oerr)?;
    for k in keys {
        expect_kind(c.op.stat(k).await, &[ErrorKind::NotFound], "NotFound")?;
    }
    Ok(())
}

fn files(entries: Vec<opendal::Entry>) -> Vec<opendal::Entry> {
    entries.into_iter().filter(|e| e.metadata().mode() == EntryMode::FILE).collect()
}

async fn list_v2_basic(c: C) -> Result<()> {
    let mut want = vec![];
    for (i, k) in ["lb/c", "lb/a", "lb/b"].iter().enumerate() {
        let data = vec![b'x'; i * 7 + 1];
        put(&c, k, &data).await?;
        want.push((k.to_string(), data.len() as u64, md5_hex(&data)));
    }
    want.sort();
    let got: Vec<(String, u64, String)> = files(c.op.list("lb/").await.map_err(oerr)?)
        .iter()
        .map(|e| (e.path().to_string(), e.metadata().content_length(), unquote(e.metadata().etag().unwrap_or_default())))
        .collect();
    check!(got == want, "got {got:?}, want {want:?}");
    Ok(())
}

async fn list_v2_prefix_delimiter(c: C) -> Result<()> {
    let b = format!("{}-d", c.b);
    aws_create_bucket(&c.admin, &b).await?;
    let res = async {
        let op = operator(&c.env, &b, &c.env.sk, false)?;
        for k in ["a/1", "a/2", "b/1", "c"] {
            op.write(k, b"x".to_vec()).await.map_err(oerr)?;
        }
        let entries = op.list("/").await.map_err(oerr)?;
        let dirs: Vec<_> =
            entries.iter().filter(|e| e.metadata().mode() == EntryMode::DIR && e.path() != "/").map(|e| e.path()).collect();
        let keys: Vec<_> = entries.iter().filter(|e| e.metadata().mode() == EntryMode::FILE).map(|e| e.path()).collect();
        check!(dirs == ["a/", "b/"], "CommonPrefixes {dirs:?}");
        check!(keys == ["c"], "Contents {keys:?}");
        Ok(())
    }
    .await;
    aws_destroy_bucket(&c.admin, &b).await;
    res
}

async fn list_v2_pagination(c: C) -> Result<()> {
    // opendal pages internally; `limit` sets max-keys per request, page boundaries are not observable
    let keys: Vec<String> = (0..25).map(|i| format!("pg/k{i:02}")).collect();
    futures::future::try_join_all(keys.iter().map(|k| put(&c, k, b"p"))).await?;
    let got: Vec<String> =
        files(c.op.list_with("pg/").limit(10).await.map_err(oerr)?).iter().map(|e| e.path().to_string()).collect();
    check!(got == keys, "got {} keys: {:?}", got.len(), got);
    Ok(())
}

async fn list_v2_start_after(c: C) -> Result<()> {
    for i in 0..5 {
        put(&c, &format!("sa/k{i}"), b"s").await?;
    }
    let got: Vec<String> = files(c.op.list_with("sa/").start_after("sa/k2").await.map_err(oerr)?)
        .iter()
        .map(|e| e.path().to_string())
        .collect();
    check!(got == ["sa/k3", "sa/k4"], "keys {got:?}");
    Ok(())
}

async fn list_v1(c: C) -> Result<()> {
    for i in 0..5 {
        put(&c, &format!("v1/k{i}"), b"v").await?;
    }
    let op = operator(&c.env, &c.b, &c.env.sk, true)?;
    let got: Vec<String> =
        files(op.list_with("v1/").start_after("v1/k1").await.map_err(oerr)?).iter().map(|e| e.path().to_string()).collect();
    check!(got == ["v1/k2", "v1/k3", "v1/k4"], "marker listing {got:?}");
    let got: Vec<String> =
        files(op.list_with("v1/").limit(2).await.map_err(oerr)?).iter().map(|e| e.path().to_string()).collect();
    check!(got.len() == 5, "paged v1 listing returned {got:?}");
    Ok(())
}

async fn copy_object(c: C) -> Result<()> {
    c.op.write_with("copy-src", b"copy me".to_vec())
        .content_type("text/x-copy")
        .user_metadata([("color".to_string(), "Blue".to_string())])
        .await
        .map_err(oerr)?;
    c.op.copy("copy-src", "copy-dst").await.map_err(oerr)?;
    let m = c.op.stat("copy-dst").await.map_err(oerr)?;
    check!(m.content_type() == Some("text/x-copy"), "Content-Type {:?}", m.content_type());
    check!(
        m.user_metadata().and_then(|u| u.get("color")) == Some("Blue"),
        "metadata {:?}",
        m.user_metadata()
    );
    check!(get(&c, "copy-dst").await? == b"copy me", "body mismatch");
    Ok(())
}

async fn multipart_upload(c: C) -> Result<()> {
    let parts = vec![rand_bytes(5 << 20), rand_bytes(5 << 20), rand_bytes(1024)];
    let mut w = c.op.writer_with("mpu.bin").chunk(5 << 20).await.map_err(oerr)?;
    for p in &parts {
        w.write(p.clone()).await.map_err(oerr)?;
    }
    w.close().await.map_err(oerr)?;
    let etag = c.op.stat("mpu.bin").await.map_err(oerr)?.etag().unwrap_or_default().to_string();
    check!(is_multipart_etag(&etag, 3), "ETag {etag} not <hex>-3");
    check!(get(&c, "mpu.bin").await? == parts.concat(), "body mismatch");
    Ok(())
}

async fn high_level_upload(c: C) -> Result<()> {
    let data = rand_bytes(20 << 20);
    let mut w = c.op.writer_with("hl.bin").chunk(8 << 20).concurrent(4).await.map_err(oerr)?;
    w.write(data.clone()).await.map_err(oerr)?;
    w.close().await.map_err(oerr)?;
    check!(get(&c, "hl.bin").await? == data, "20MiB body mismatch");
    Ok(())
}

async fn presigned_get(c: C) -> Result<()> {
    put(&c, "presign-get.txt", b"presigned get").await?;
    let p = c.op.presign_read("presign-get.txt", Duration::from_secs(600)).await.map_err(oerr)?;
    let mut req = http().get(p.uri().to_string());
    for (k, v) in p.header() {
        req = req.header(k.as_str(), v.to_str()?);
    }
    let r = req.send().await?;
    let st = r.status();
    let body = r.bytes().await?;
    check!(st.as_u16() == 200, "HTTP {st}: {}", String::from_utf8_lossy(&body));
    check!(&body[..] == b"presigned get", "body mismatch");
    Ok(())
}

async fn presigned_put(c: C) -> Result<()> {
    let p = c.op.presign_write("presign-put.txt", Duration::from_secs(600)).await.map_err(oerr)?;
    let mut req = http().put(p.uri().to_string()).body("presigned put");
    for (k, v) in p.header() {
        req = req.header(k.as_str(), v.to_str()?);
    }
    let r = req.send().await?;
    let st = r.status();
    check!(st.is_success(), "HTTP {st}: {}", r.text().await.unwrap_or_default());
    check!(get(&c, "presign-put.txt").await? == b"presigned put", "body mismatch");
    Ok(())
}

async fn bad_credentials(c: C) -> Result<()> {
    // opendal has no ListBuckets; use ListObjectsV2 on the bucket instead
    let op = operator(&c.env, &c.b, "definitely-wrong-secret", false)?;
    expect_kind(op.list("/").await, &[ErrorKind::PermissionDenied], "403 PermissionDenied")
}

pub async fn run(env: &Env) -> Report {
    let b = bucket_name("opendal");
    let admin = aws_client(env, &env.sk);
    let mut r = Report::new(SDK, &crate::versions::lock_version("opendal"));
    if let Err(e) = aws_create_bucket(&admin, &b).await {
        eprintln!("bucket setup via aws-sdk-s3 failed: {e:#}");
    }
    let op = operator(env, &b, &env.sk, false).expect("build operator");
    let c = Arc::new(Ctx { env: env.clone(), op, b: b.clone(), admin: admin.clone() });
    macro_rules! sc {
        ($name:ident) => {
            r.run(stringify!($name), $name(c.clone())).await
        };
    }
    macro_rules! sk {
        ($name:ident, $why:expr) => {
            r.run(stringify!($name), async { Err(skip($why)) }).await
        };
    }
    sk!(create_bucket, format!("{NO_BUCKET_API} (bucket created via aws-sdk-s3)"));
    sk!(head_bucket, NO_BUCKET_API);
    sk!(head_bucket_missing, NO_BUCKET_API);
    sk!(list_buckets, NO_BUCKET_API);
    sc!(put_get_small);
    sc!(put_get_empty);
    sc!(put_get_large);
    sc!(special_keys);
    sc!(head_object_metadata);
    sc!(get_range);
    sc!(get_range_invalid);
    sc!(get_missing_key);
    sc!(get_if_none_match);
    sc!(get_if_match_fail);
    sc!(put_if_none_match);
    sc!(delete_object);
    sc!(delete_missing_object);
    sc!(delete_objects);
    sc!(list_v2_basic);
    sc!(list_v2_prefix_delimiter);
    sc!(list_v2_pagination);
    sc!(list_v2_start_after);
    sc!(list_v1);
    sc!(copy_object);
    sk!(copy_object_replace_metadata, "opendal copy has no metadata directive / replacement metadata");
    sc!(multipart_upload);
    sk!(multipart_abort, "opendal has no ListParts API to verify the abort");
    sk!(multipart_list_parts, "opendal has no ListParts API");
    sk!(multipart_too_small, "opendal clamps multipart chunk size to >= 5 MiB (write_multi_min_size), cannot upload undersized parts");
    sc!(high_level_upload);
    sc!(presigned_get);
    sc!(presigned_put);
    for s in ["checksum_crc32", "checksum_crc32c", "checksum_sha1", "checksum_sha256", "checksum_crc64nvme"] {
        r.run(s, async { Err(skip("opendal cannot request ChecksumMode or read response checksums on GET")) }).await;
    }
    sk!(content_md5_bad, "opendal has no Content-MD5 option for writes");
    sc!(bad_credentials);
    sk!(delete_bucket_not_empty, NO_BUCKET_API);
    sk!(versioning, "opendal cannot enable bucket versioning");
    sk!(object_tagging, "opendal has no object tagging API");
    sk!(delete_bucket, NO_BUCKET_API);
    aws_destroy_bucket(&admin, &b).await;
    r
}
