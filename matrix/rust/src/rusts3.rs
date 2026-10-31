use std::sync::Arc;

use anyhow::{anyhow, Result};
use s3::creds::Credentials;
use s3::error::S3Error;
use s3::serde_types::ObjectIdentifier;
use s3::{Bucket, BucketConfiguration, Region};

use crate::check;
use crate::common::*;

pub const SDK: &str = "rust-s3";

pub struct Ctx {
    pub env: Env,
    pub bucket: Box<Bucket>,
    pub b: String,
}
type C = Arc<Ctx>;

fn region(env: &Env) -> Region {
    Region::Custom { region: env.region.clone(), endpoint: env.endpoint.clone() }
}

fn creds(env: &Env, secret: &str) -> Result<Credentials> {
    Ok(Credentials::new(Some(&env.ak), Some(secret), None, None, None)?)
}

fn bucket(env: &Env, name: &str) -> Result<Box<Bucket>> {
    Ok(Bucket::new(name, region(env), creds(env, &env.sk)?)?.with_path_style())
}

fn xml_tag<'a>(body: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let i = body.find(&open)? + open.len();
    let j = body[i..].find(&format!("</{tag}>"))?;
    Some(&body[i..i + j])
}

/// (HTTP status, S3 error code) of a rust-s3 error
fn status_code(e: &S3Error) -> (Option<u16>, Option<String>) {
    match e {
        S3Error::HttpFailWithBody(st, body) => (Some(*st), xml_tag(body, "Code").map(String::from)),
        _ => (None, None),
    }
}

fn serr(e: S3Error) -> anyhow::Error {
    match &e {
        S3Error::HttpFailWithBody(st, body) => anyhow!(
            "{}: {} (HTTP {st})",
            xml_tag(body, "Code").unwrap_or("HttpFailWithBody"),
            xml_tag(body, "Message").unwrap_or(body.lines().next().unwrap_or(""))
        ),
        _ => {
            let dbg = format!("{e:?}");
            anyhow!("S3Error::{}: {e}", dbg.split(['(', ' ', '{']).next().unwrap_or("?"))
        }
    }
}

fn expect_err<T>(r: Result<T, S3Error>, statuses: &[u16], codes: &[&str]) -> Result<()> {
    match r {
        Ok(_) => Err(anyhow!("AssertionError: expected {codes:?}/{statuses:?}, got success")),
        Err(e) => {
            let (st, code) = status_code(&e);
            if st.map(|s| statuses.contains(&s)).unwrap_or(false)
                || code.as_deref().map(|c| codes.contains(&c)).unwrap_or(false)
            {
                Ok(())
            } else {
                Err(anyhow!("AssertionError: expected {codes:?}/{statuses:?}, got {}", serr(e)))
            }
        }
    }
}

fn with_headers(b: &Bucket, hs: &[(&str, &str)]) -> Result<Bucket> {
    let mut m = http::HeaderMap::new();
    for (k, v) in hs {
        m.insert(http::HeaderName::from_bytes(k.as_bytes())?, http::HeaderValue::from_str(v)?);
    }
    Ok(b.with_extra_headers(m)?)
}

async fn put(c: &C, key: &str, data: &[u8]) -> Result<String> {
    let r = c.bucket.put_object(key, data).await.map_err(serr)?;
    Ok(unquote(r.headers().get("etag").map(String::as_str).unwrap_or_default()))
}

async fn get(c: &C, key: &str) -> Result<Vec<u8>> {
    Ok(c.bucket.get_object(key).await.map_err(serr)?.to_vec())
}

async fn list_keys(b: &Bucket, prefix: &str) -> Result<Vec<s3::serde_types::Object>> {
    Ok(b.list(prefix.to_string(), None).await.map_err(serr)?.into_iter().flat_map(|p| p.contents).collect())
}

async fn create_bucket(c: C) -> Result<()> {
    let r = Bucket::create_with_path_style(&c.b, region(&c.env), creds(&c.env, &c.env.sk)?, BucketConfiguration::default())
        .await
        .map_err(serr)?;
    check!(r.success(), "create bucket HTTP {}: {}", r.response_code, r.response_text);
    Ok(())
}

async fn list_buckets(c: C) -> Result<()> {
    let r = Bucket::list_buckets(region(&c.env), creds(&c.env, &c.env.sk)?).await.map_err(serr)?;
    let names: Vec<String> = r.bucket_names().collect();
    check!(names.contains(&c.b), "bucket {} not in {names:?}", c.b);
    Ok(())
}

async fn put_get_small(c: C) -> Result<()> {
    let data = b"hello world";
    let etag = put(&c, "small.txt", data).await?;
    check!(etag == md5_hex(data), "put ETag {etag} != md5");
    let r = c.bucket.get_object("small.txt").await.map_err(serr)?;
    check!(r.as_slice() == data, "body mismatch");
    let getag = unquote(r.headers().get("etag").map(String::as_str).unwrap_or_default());
    check!(getag == md5_hex(data), "get ETag {getag} != md5");
    Ok(())
}

async fn put_get_empty(c: C) -> Result<()> {
    put(&c, "empty", b"").await?;
    let r = c.bucket.get_object("empty").await.map_err(serr)?;
    check!(r.as_slice().is_empty(), "body len {}", r.as_slice().len());
    let cl = r.headers().get("content-length").cloned();
    check!(cl.as_deref() == Some("0"), "Content-Length {cl:?}");
    Ok(())
}

async fn put_get_large(c: C) -> Result<()> {
    let data = rand_bytes(8 << 20);
    put(&c, "large.bin", &data).await?;
    check!(get(&c, "large.bin").await? == data, "8MiB body mismatch");
    Ok(())
}

async fn special_keys(c: C) -> Result<()> {
    for (i, k) in SPECIAL_KEYS.iter().enumerate() {
        let data = format!("special-{i}");
        put(&c, k, data.as_bytes()).await?;
        check!(get(&c, k).await? == data.as_bytes(), "body mismatch for {k:?}");
    }
    let keys: Vec<String> = list_keys(&c.bucket, "").await?.into_iter().map(|o| o.key).collect();
    for k in SPECIAL_KEYS {
        check!(keys.iter().any(|x| x == k), "key {k:?} not listed");
    }
    Ok(())
}

async fn head_object_metadata(c: C) -> Result<()> {
    let r = c
        .bucket
        .put_object_builder("meta.txt", b"metadata")
        .with_content_type("text/x-matrix")
        .with_cache_control("max-age=3600")?
        .with_content_disposition("attachment; filename=\"m.txt\"")?
        .with_content_encoding("gzip")?
        .with_metadata("color", "Blue")?
        .with_metadata("x-y", "z")?
        .execute()
        .await
        .map_err(serr)?;
    check!(r.status_code() / 100 == 2, "put HTTP {}", r.status_code());
    let (h, _) = c.bucket.head_object("meta.txt").await.map_err(serr)?;
    check!(h.content_type.as_deref() == Some("text/x-matrix"), "Content-Type {:?}", h.content_type);
    check!(h.cache_control.as_deref() == Some("max-age=3600"), "Cache-Control {:?}", h.cache_control);
    check!(
        h.content_disposition.as_deref() == Some("attachment; filename=\"m.txt\""),
        "Content-Disposition {:?}",
        h.content_disposition
    );
    check!(h.content_encoding.as_deref() == Some("gzip"), "Content-Encoding {:?}", h.content_encoding);
    let want: std::collections::HashMap<String, String> =
        [("color", "Blue"), ("x-y", "z")].iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
    check!(h.metadata.as_ref() == Some(&want), "metadata {:?}", h.metadata);
    Ok(())
}

async fn get_range(c: C) -> Result<()> {
    let data = range_body();
    put(&c, "range.bin", &data).await?;
    let check_resp = |label: &str, r: &s3::request::ResponseData, lo: usize, hi: usize| -> Result<()> {
        check!(r.as_slice() == &data[lo..=hi], "{label}: body mismatch ({} bytes)", r.as_slice().len());
        let cr = r.headers().get("content-range").cloned();
        let want = format!("bytes {lo}-{hi}/100");
        check!(cr.as_deref() == Some(want.as_str()), "{label}: Content-Range {cr:?} != {want}");
        Ok(())
    };
    let r = c.bucket.get_object_range("range.bin", 10, Some(19)).await.map_err(serr)?;
    check_resp("bytes=10-19", &r, 10, 19)?;
    // rust-s3 has no suffix-range API; pass the Range header through the bucket's extra headers
    let r = with_headers(&c.bucket, &[("range", "bytes=-5")])?.get_object("range.bin").await.map_err(serr)?;
    check_resp("bytes=-5", &r, 95, 99)?;
    let r = c.bucket.get_object_range("range.bin", 95, None).await.map_err(serr)?;
    check_resp("bytes=95-", &r, 95, 99)?;
    Ok(())
}

async fn get_range_invalid(c: C) -> Result<()> {
    put(&c, "range-inv.bin", &range_body()).await?;
    expect_err(c.bucket.get_object_range("range-inv.bin", 200, Some(300)).await, &[416], &["InvalidRange"])
}

async fn get_missing_key(c: C) -> Result<()> {
    expect_err(c.bucket.get_object("does/not/exist").await, &[404], &["NoSuchKey"])
}

async fn get_if_none_match(c: C) -> Result<()> {
    let etag = put(&c, "inm.txt", b"if-none-match").await?;
    let b = with_headers(&c.bucket, &[("if-none-match", &format!("\"{etag}\""))])?;
    expect_err(b.get_object("inm.txt").await, &[304], &[])
}

async fn get_if_match_fail(c: C) -> Result<()> {
    put(&c, "im.txt", b"if-match").await?;
    let b = with_headers(&c.bucket, &[("if-match", "\"wrong\"")])?;
    expect_err(b.get_object("im.txt").await, &[412], &[])
}

async fn put_if_none_match(c: C) -> Result<()> {
    put(&c, "pinm.txt", b"first").await?;
    let mut h = http::HeaderMap::new();
    h.insert("if-none-match", http::HeaderValue::from_static("*"));
    expect_err(c.bucket.put_object_with_headers("pinm.txt", b"second", Some(h.clone())).await, &[412], &[])?;
    let k = format!("pinm-new-{}", rand_hex8());
    c.bucket.put_object_with_headers(&k, b"new", Some(h)).await.map_err(serr)?;
    check!(get(&c, &k).await? == b"new", "new key body mismatch");
    check!(get(&c, "pinm.txt").await? == b"first", "existing key overwritten");
    Ok(())
}

async fn delete_object(c: C) -> Result<()> {
    put(&c, "del.txt", b"x").await?;
    c.bucket.delete_object("del.txt").await.map_err(serr)?;
    expect_err(c.bucket.head_object("del.txt").await, &[404], &["NoSuchKey", "NotFound"])
}

async fn delete_missing_object(c: C) -> Result<()> {
    let r = c.bucket.delete_object(format!("never-existed-{}", rand_hex8())).await.map_err(serr)?;
    check!(r.status_code() == 204, "HTTP {}", r.status_code());
    Ok(())
}

async fn delete_objects(c: C) -> Result<()> {
    let keys = ["bulk/1", "bulk/2", "bulk/3"];
    for k in keys {
        put(&c, k, k.as_bytes()).await?;
    }
    let ids: Vec<ObjectIdentifier> = keys.iter().map(|k| ObjectIdentifier::new(*k)).collect();
    let r = c.bucket.delete_objects(ids).await.map_err(serr)?;
    check!(r.errors.is_empty(), "errors {:?}", r.errors);
    check!(r.deleted.len() == 3, "deleted {} entries", r.deleted.len());
    let left = list_keys(&c.bucket, "bulk/").await?;
    check!(left.is_empty(), "{} keys remain", left.len());
    Ok(())
}

async fn list_v2_basic(c: C) -> Result<()> {
    let mut want = vec![];
    for (i, k) in ["lb/c", "lb/a", "lb/b"].iter().enumerate() {
        let data = vec![b'x'; i * 7 + 1];
        put(&c, k, &data).await?;
        want.push((k.to_string(), data.len() as u64, md5_hex(&data)));
    }
    want.sort();
    let (r, _) = c.bucket.list_page("lb/".into(), None, None, None, None).await.map_err(serr)?;
    let got: Vec<(String, u64, String)> =
        r.contents.iter().map(|o| (o.key.clone(), o.size, unquote(o.e_tag.as_deref().unwrap_or_default()))).collect();
    check!(got == want, "got {got:?}, want {want:?}");
    Ok(())
}

async fn list_v2_prefix_delimiter(c: C) -> Result<()> {
    let name = format!("{}-d", c.b);
    // auxiliary bucket created via aws-sdk-s3 so this scenario does not depend on create_bucket
    aws_create_bucket(&aws_client(&c.env, &c.env.sk), &name).await?;
    let b = bucket(&c.env, &name)?;
    let res = async {
        for k in ["a/1", "a/2", "b/1", "c"] {
            b.put_object(k, b"x").await.map_err(serr)?;
        }
        let (r, _) = b.list_page(String::new(), Some("/".into()), None, None, None).await.map_err(serr)?;
        let cps: Vec<String> = r.common_prefixes.unwrap_or_default().into_iter().map(|p| p.prefix).collect();
        let keys: Vec<String> = r.contents.into_iter().map(|o| o.key).collect();
        check!(cps == ["a/", "b/"], "CommonPrefixes {cps:?}");
        check!(keys == ["c"], "Contents {keys:?}");
        Ok(())
    }
    .await;
    for k in ["a/1", "a/2", "b/1", "c"] {
        let _ = b.delete_object(k).await;
    }
    let _ = b.delete().await;
    res
}

async fn list_v2_pagination(c: C) -> Result<()> {
    let keys: Vec<String> = (0..25).map(|i| format!("pg/k{i:02}")).collect();
    futures::future::try_join_all(keys.iter().map(|k| put(&c, k, b"p"))).await?;
    let mut pages = 0;
    let mut got = vec![];
    let mut token = None;
    loop {
        let (r, _) = c.bucket.list_page("pg/".into(), None, token, None, Some(10)).await.map_err(serr)?;
        pages += 1;
        check!(r.contents.len() <= 10, "page {pages} has {} keys", r.contents.len());
        got.extend(r.contents.into_iter().map(|o| o.key));
        token = r.next_continuation_token;
        if !r.is_truncated || token.is_none() || pages > 10 {
            break;
        }
    }
    check!(pages == 3, "{pages} pages");
    check!(got == keys, "keys mismatch: got {} keys", got.len());
    Ok(())
}

async fn list_v2_start_after(c: C) -> Result<()> {
    for i in 0..5 {
        put(&c, &format!("sa/k{i}"), b"s").await?;
    }
    let (r, _) = c.bucket.list_page("sa/".into(), None, None, Some("sa/k2".into()), None).await.map_err(serr)?;
    let keys: Vec<String> = r.contents.into_iter().map(|o| o.key).collect();
    check!(keys == ["sa/k3", "sa/k4"], "keys {keys:?}");
    Ok(())
}

async fn list_v1(c: C) -> Result<()> {
    for i in 0..5 {
        put(&c, &format!("v1/k{i}"), b"v").await?;
    }
    let b = c.bucket.with_listobjects_v1();
    // in v1 mode rust-s3 sends start_after as Marker
    let (r, _) = b.list_page("v1/".into(), None, None, Some("v1/k1".into()), None).await.map_err(serr)?;
    let keys: Vec<String> = r.contents.into_iter().map(|o| o.key).collect();
    check!(keys == ["v1/k2", "v1/k3", "v1/k4"], "keys {keys:?}");
    let (r, _) = b.list_page("v1/".into(), None, None, None, Some(2)).await.map_err(serr)?;
    let keys: Vec<String> = r.contents.into_iter().map(|o| o.key).collect();
    check!(keys == ["v1/k0", "v1/k1"], "page1 {keys:?}");
    check!(r.is_truncated, "IsTruncated false");
    Ok(())
}

async fn copy_object(c: C) -> Result<()> {
    c.bucket
        .put_object_builder("copy-src", b"copy me")
        .with_content_type("text/x-copy")
        .with_metadata("color", "Blue")?
        .execute()
        .await
        .map_err(serr)?;
    c.bucket.copy_object_internal("copy-src", "copy-dst").await.map_err(serr)?;
    let (h, _) = c.bucket.head_object("copy-dst").await.map_err(serr)?;
    check!(h.content_type.as_deref() == Some("text/x-copy"), "Content-Type {:?}", h.content_type);
    check!(
        h.metadata.as_ref().and_then(|m| m.get("color")).map(String::as_str) == Some("Blue"),
        "metadata {:?}",
        h.metadata
    );
    check!(get(&c, "copy-dst").await? == b"copy me", "body mismatch");
    Ok(())
}

async fn copy_object_replace_metadata(c: C) -> Result<()> {
    c.bucket
        .put_object_builder("crm-src", b"replace")
        .with_metadata("old", "1")?
        .execute()
        .await
        .map_err(serr)?;
    // rust-s3's copy has no metadata options; use the bucket's extra headers
    let b = with_headers(&c.bucket, &[("x-amz-metadata-directive", "REPLACE"), ("x-amz-meta-new", "2")])?;
    b.copy_object_internal("crm-src", "crm-dst").await.map_err(serr)?;
    let (h, _) = c.bucket.head_object("crm-dst").await.map_err(serr)?;
    let want: std::collections::HashMap<String, String> = [("new".to_string(), "2".to_string())].into();
    check!(h.metadata.as_ref() == Some(&want), "metadata {:?}", h.metadata);
    check!(get(&c, "crm-dst").await? == b"replace", "body mismatch");
    Ok(())
}

async fn mpu(c: &C, key: &str, parts: &[Vec<u8>]) -> Result<(String, Vec<s3::serde_types::Part>)> {
    let up = c.bucket.initiate_multipart_upload(key, "application/octet-stream").await.map_err(serr)?;
    let mut done = vec![];
    for (i, p) in parts.iter().enumerate() {
        done.push(
            c.bucket
                .put_multipart_chunk(p.clone(), key, i as u32 + 1, &up.upload_id, "application/octet-stream")
                .await
                .map_err(serr)?,
        );
    }
    Ok((up.upload_id, done))
}

async fn multipart_upload(c: C) -> Result<()> {
    let parts = vec![rand_bytes(5 << 20), rand_bytes(5 << 20), rand_bytes(1024)];
    let (id, done) = mpu(&c, "mpu.bin", &parts).await?;
    let r = c.bucket.complete_multipart_upload("mpu.bin", &id, done).await.map_err(serr)?;
    let body = String::from_utf8_lossy(r.as_slice()).to_string();
    check!(!body.contains("<Error>"), "complete returned error body: {body}");
    let etag = xml_tag(&body, "ETag").map(|s| s.replace("&quot;", "\"").replace("&#34;", "\"")).unwrap_or_default();
    check!(is_multipart_etag(&etag, 3), "ETag {etag:?} not <hex>-3");
    check!(get(&c, "mpu.bin").await? == parts.concat(), "body mismatch");
    Ok(())
}

async fn multipart_too_small(c: C) -> Result<()> {
    let (id, done) = mpu(&c, "mpu-small.bin", &[rand_bytes(1024), rand_bytes(1024)]).await?;
    let r = c.bucket.complete_multipart_upload("mpu-small.bin", &id, done).await;
    let _ = c.bucket.abort_upload("mpu-small.bin", &id).await;
    match r {
        Err(e) => {
            let (_, code) = status_code(&e);
            check!(code.as_deref() == Some("EntityTooSmall"), "expected EntityTooSmall, got {}", serr(e));
            Ok(())
        }
        // S3 may report errors inside a 200 response body
        Ok(r) => {
            let body = String::from_utf8_lossy(r.as_slice()).to_string();
            check!(xml_tag(&body, "Code") == Some("EntityTooSmall"), "expected EntityTooSmall, got success: {body}");
            Ok(())
        }
    }
}

async fn high_level_upload(c: C) -> Result<()> {
    let data = rand_bytes(20 << 20);
    let mut cur = std::io::Cursor::new(data.clone());
    let r = c.bucket.put_object_stream(&mut cur, "hl.bin").await.map_err(serr)?;
    check!(r.status_code() / 100 == 2, "HTTP {}", r.status_code());
    check!(get(&c, "hl.bin").await? == data, "20MiB body mismatch");
    Ok(())
}

async fn presigned_get(c: C) -> Result<()> {
    put(&c, "presign-get.txt", b"presigned get").await?;
    let url = c.bucket.presign_get("presign-get.txt", 600, None).await.map_err(serr)?;
    let r = http().get(url).send().await?;
    let st = r.status();
    let body = r.bytes().await?;
    check!(st.as_u16() == 200, "HTTP {st}: {}", String::from_utf8_lossy(&body));
    check!(&body[..] == b"presigned get", "body mismatch");
    Ok(())
}

async fn presigned_put(c: C) -> Result<()> {
    let url = c.bucket.presign_put("presign-put.txt", 600, None, None).await.map_err(serr)?;
    let r = http().put(url).body("presigned put").send().await?;
    let st = r.status();
    check!(st.is_success(), "HTTP {st}: {}", r.text().await.unwrap_or_default());
    check!(get(&c, "presign-put.txt").await? == b"presigned put", "body mismatch");
    Ok(())
}

async fn bad_credentials(c: C) -> Result<()> {
    let r = Bucket::list_buckets(region(&c.env), creds(&c.env, "definitely-wrong-secret")?).await;
    expect_err(r, &[403], &["SignatureDoesNotMatch", "InvalidAccessKeyId"])
}

async fn delete_bucket_not_empty(c: C) -> Result<()> {
    put(&c, "keep-bucket-nonempty", b"x").await?;
    expect_err(c.bucket.delete().await, &[409], &["BucketNotEmpty"])
}

async fn object_tagging(c: C) -> Result<()> {
    put(&c, "tagged", b"t").await?;
    c.bucket.put_object_tagging("tagged", &[("env", "test"), ("team", "s3 matrix")]).await.map_err(serr)?;
    let (tags, _) = c.bucket.get_object_tagging("tagged").await.map_err(serr)?;
    let mut got: Vec<(String, String)> = tags.iter().map(|t| (t.key(), t.value())).collect();
    got.sort();
    let want = vec![("env".to_string(), "test".to_string()), ("team".to_string(), "s3 matrix".to_string())];
    check!(got == want, "tags {got:?}");
    Ok(())
}

async fn empty_bucket(b: &Bucket) -> Result<()> {
    let objs = list_keys(b, "").await?;
    for chunk in objs.chunks(500) {
        let ids: Vec<ObjectIdentifier> = chunk.iter().map(|o| ObjectIdentifier::new(o.key.clone())).collect();
        b.delete_objects(ids).await.map_err(serr)?;
    }
    if let Ok(ups) = b.list_multiparts_uploads(None, None).await {
        for page in ups {
            for u in page.uploads {
                let _ = b.abort_upload(&u.key, &u.id).await;
            }
        }
    }
    Ok(())
}

async fn delete_bucket(c: C) -> Result<()> {
    empty_bucket(&c.bucket).await?;
    let code = c.bucket.delete().await.map_err(serr)?;
    check!(code / 100 == 2, "DeleteBucket HTTP {code}");
    let r = Bucket::list_buckets(region(&c.env), creds(&c.env, &c.env.sk)?).await.map_err(serr)?;
    check!(!r.bucket_names().any(|n| n == c.b), "bucket still listed after delete");
    // rust-s3 has no HeadBucket; verify with a request that must now fail with NoSuchBucket
    expect_err(c.bucket.list_page(String::new(), None, None, None, None).await, &[404], &["NoSuchBucket"])
}

pub async fn run(env: &Env) -> Report {
    let b = bucket_name("rust-s3");
    let mut r = Report::new(SDK, &crate::versions::lock_version("rust-s3"));
    let c = Arc::new(Ctx { env: env.clone(), bucket: bucket(env, &b).expect("bucket handle"), b: b.clone() });
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
    sc!(create_bucket);
    // if rust-s3 could not create the bucket, create it via aws-sdk-s3 so the other scenarios still run
    let admin = aws_client(env, &env.sk);
    if admin.head_bucket().bucket(&b).send().await.is_err() {
        let _ = aws_create_bucket(&admin, &b).await;
    }
    sk!(head_bucket, "rust-s3 has no HeadBucket API (Bucket::exists uses ListBuckets)");
    sk!(head_bucket_missing, "rust-s3 has no HeadBucket API (Bucket::exists uses ListBuckets)");
    sc!(list_buckets);
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
    sc!(copy_object_replace_metadata);
    sc!(multipart_upload);
    sk!(multipart_abort, "rust-s3 has no ListParts API to verify the abort");
    sk!(multipart_list_parts, "rust-s3 has no ListParts API");
    sc!(multipart_too_small);
    sc!(high_level_upload);
    sc!(presigned_get);
    sc!(presigned_put);
    for s in ["checksum_crc32", "checksum_crc32c", "checksum_sha1", "checksum_sha256", "checksum_crc64nvme"] {
        r.run(s, async { Err(skip("rust-s3 has no flexible-checksum (x-amz-checksum-*) support")) }).await;
    }
    sk!(content_md5_bad, "rust-s3 always overwrites Content-MD5 with the computed digest");
    sc!(bad_credentials);
    sc!(delete_bucket_not_empty);
    sk!(versioning, "rust-s3 has no bucket versioning / ListObjectVersions API");
    sc!(object_tagging);
    sc!(delete_bucket);
    // best-effort cleanup if delete_bucket failed
    let _ = empty_bucket(&c.bucket).await;
    let _ = c.bucket.delete().await;
    r
}
