use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use futures::{StreamExt, TryStreamExt};
use object_store::aws::{AmazonS3, AmazonS3Builder};
use object_store::list::{PaginatedListOptions, PaginatedListStore};
use object_store::multipart::MultipartStore;
use object_store::path::Path;
use object_store::signer::Signer;
use object_store::{
    Attribute, Attributes, GetOptions, GetRange, ObjectStore, ObjectStoreExt, PutMode, PutOptions, PutPayload,
    RetryConfig,
};
use tokio::io::AsyncWriteExt;

use crate::check;
use crate::common::*;

pub const SDK: &str = "object_store";

pub struct Ctx {
    pub env: Env,
    pub s: Arc<AmazonS3>,
    pub b: String,
    pub admin: aws_sdk_s3::Client,
}
type C = Arc<Ctx>;

fn store(env: &Env, bucket: &str, secret: &str) -> Result<AmazonS3> {
    Ok(AmazonS3Builder::new()
        .with_endpoint(&env.endpoint)
        .with_allow_http(true)
        .with_virtual_hosted_style_request(false)
        .with_region(&env.region)
        .with_bucket_name(bucket)
        .with_access_key_id(&env.ak)
        .with_secret_access_key(secret)
        .with_retry(RetryConfig { max_retries: 1, retry_timeout: Duration::from_secs(20), ..Default::default() })
        .build()?)
}

fn oerr(e: object_store::Error) -> anyhow::Error {
    let kind = format!("{e:?}");
    let kind = kind.split([' ', '{', '(']).next().unwrap_or("Error").to_string();
    anyhow!("object_store::Error::{kind}: {e}")
}

fn p(s: &str) -> Result<Path> {
    Ok(Path::parse(s)?)
}

async fn put(c: &C, key: &str, data: &[u8]) -> Result<String> {
    let r = c.s.put(&p(key)?, PutPayload::from(data.to_vec())).await.map_err(oerr)?;
    Ok(unquote(r.e_tag.as_deref().unwrap_or_default()))
}

async fn get(c: &C, key: &str) -> Result<Vec<u8>> {
    Ok(c.s.get(&p(key)?).await.map_err(oerr)?.bytes().await.map_err(oerr)?.to_vec())
}

const NO_BUCKET_API: &str = "object_store has no bucket management API";

async fn put_get_small(c: C) -> Result<()> {
    let data = b"hello world";
    let etag = put(&c, "small.txt", data).await?;
    check!(etag == md5_hex(data), "put ETag {etag} != md5");
    let r = c.s.get(&p("small.txt")?).await.map_err(oerr)?;
    let getag = unquote(r.meta.e_tag.as_deref().unwrap_or_default());
    let body = r.bytes().await.map_err(oerr)?;
    check!(&body[..] == data, "body mismatch");
    check!(getag == md5_hex(data), "get ETag {getag} != md5");
    Ok(())
}

async fn put_get_empty(c: C) -> Result<()> {
    put(&c, "empty", b"").await?;
    let r = c.s.get(&p("empty")?).await.map_err(oerr)?;
    check!(r.meta.size == 0, "size {}", r.meta.size);
    let body = r.bytes().await.map_err(oerr)?;
    check!(body.is_empty(), "body len {}", body.len());
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
        if Path::parse(k).is_err() {
            continue;
        }
        let data = format!("special-{i}");
        put(&c, k, data.as_bytes()).await?;
        check!(get(&c, k).await? == data.as_bytes(), "body mismatch for {k:?}");
        tested.push(*k);
    }
    let listed: Vec<String> = c.s.list(None).map_ok(|m| m.location.to_string()).try_collect().await.map_err(oerr)?;
    for k in &tested {
        check!(listed.iter().any(|x| x == k), "key {k:?} not listed");
    }
    if tested.len() < SPECIAL_KEYS.len() {
        return Err(skip(format!(
            "object_store Path cannot represent key \"a//b\" (empty segment); other {} keys passed",
            tested.len()
        )));
    }
    Ok(())
}

fn meta_attrs() -> Attributes {
    let mut a = Attributes::new();
    a.insert(Attribute::ContentType, "text/x-matrix".into());
    a.insert(Attribute::CacheControl, "max-age=3600".into());
    a.insert(Attribute::ContentDisposition, "attachment; filename=\"m.txt\"".into());
    a.insert(Attribute::ContentEncoding, "gzip".into());
    a.insert(Attribute::Metadata("color".into()), "Blue".into());
    a.insert(Attribute::Metadata("x-y".into()), "z".into());
    a
}

async fn head_object_metadata(c: C) -> Result<()> {
    let opts = PutOptions { attributes: meta_attrs(), ..Default::default() };
    c.s.put_opts(&p("meta.txt")?, PutPayload::from_static(b"metadata"), opts).await.map_err(oerr)?;
    let r = c.s.get_opts(&p("meta.txt")?, GetOptions { head: true, ..Default::default() }).await.map_err(oerr)?;
    let want = meta_attrs();
    for (k, v) in want.iter() {
        let got = r.attributes.get(k);
        check!(got.map(|g| g.as_ref()) == Some(v.as_ref()), "{k:?}: got {:?}, want {:?}", got, v);
    }
    check!(r.attributes.len() == want.len(), "extra attributes: {:?}", r.attributes);
    Ok(())
}

async fn get_range(c: C) -> Result<()> {
    let data = range_body();
    put(&c, "range.bin", &data).await?;
    for (label, range, lo, hi) in [
        ("bytes=10-19", GetRange::Bounded(10..20), 10usize, 19usize),
        ("bytes=-5", GetRange::Suffix(5), 95, 99),
        ("bytes=95-", GetRange::Offset(95), 95, 99),
    ] {
        let r = c.s.get_opts(&p("range.bin")?, GetOptions { range: Some(range), ..Default::default() }).await.map_err(oerr)?;
        // object_store parses Content-Range into `range` and `meta.size`
        check!(r.range == (lo as u64..hi as u64 + 1), "{label}: Content-Range parsed as {:?}", r.range);
        check!(r.meta.size == 100, "{label}: total size {} from Content-Range", r.meta.size);
        let body = r.bytes().await.map_err(oerr)?;
        check!(&body[..] == &data[lo..=hi], "{label}: body mismatch");
    }
    Ok(())
}

async fn get_range_invalid(c: C) -> Result<()> {
    put(&c, "range-inv.bin", &range_body()).await?;
    let r = c.s.get_opts(&p("range-inv.bin")?, GetOptions { range: Some(GetRange::Bounded(200..301)), ..Default::default() }).await;
    match r {
        Ok(_) => Err(anyhow!("AssertionError: expected 416, got success")),
        Err(e) => {
            let s = e.to_string();
            check!(
                s.contains("416") || s.contains("InvalidRange") || s.contains("Range Not Satisfiable"),
                "expected 416/InvalidRange, got {}",
                oerr(e)
            );
            Ok(())
        }
    }
}

async fn get_missing_key(c: C) -> Result<()> {
    match c.s.get(&p("does/not/exist")?).await {
        Err(object_store::Error::NotFound { .. }) => Ok(()),
        Err(e) => Err(anyhow!("AssertionError: expected NotFound, got {}", oerr(e))),
        Ok(_) => Err(anyhow!("AssertionError: expected NotFound, got success")),
    }
}

async fn get_if_none_match(c: C) -> Result<()> {
    let etag = put(&c, "inm.txt", b"if-none-match").await?;
    let r = c.s.get_opts(&p("inm.txt")?, GetOptions { if_none_match: Some(format!("\"{etag}\"")), ..Default::default() }).await;
    match r {
        Err(object_store::Error::NotModified { .. }) => Ok(()),
        Err(e) => Err(anyhow!("AssertionError: expected 304 NotModified, got {}", oerr(e))),
        Ok(_) => Err(anyhow!("AssertionError: expected 304, got success")),
    }
}

async fn get_if_match_fail(c: C) -> Result<()> {
    put(&c, "im.txt", b"if-match").await?;
    let r = c.s.get_opts(&p("im.txt")?, GetOptions { if_match: Some("\"wrong\"".into()), ..Default::default() }).await;
    match r {
        Err(object_store::Error::Precondition { .. }) => Ok(()),
        Err(e) => Err(anyhow!("AssertionError: expected 412 Precondition, got {}", oerr(e))),
        Ok(_) => Err(anyhow!("AssertionError: expected 412, got success")),
    }
}

async fn put_if_none_match(c: C) -> Result<()> {
    put(&c, "pinm.txt", b"first").await?;
    let create = || PutOptions { mode: PutMode::Create, ..Default::default() };
    match c.s.put_opts(&p("pinm.txt")?, PutPayload::from_static(b"second"), create()).await {
        // object_store maps 412 on PutMode::Create to AlreadyExists
        Err(object_store::Error::AlreadyExists { .. }) | Err(object_store::Error::Precondition { .. }) => {}
        Err(e) => return Err(anyhow!("AssertionError: expected 412, got {}", oerr(e))),
        Ok(_) => return Err(anyhow!("AssertionError: expected 412, got success")),
    }
    let k = format!("pinm-new-{}", rand_hex8());
    c.s.put_opts(&p(&k)?, PutPayload::from_static(b"new"), create()).await.map_err(oerr)?;
    check!(get(&c, &k).await? == b"new", "new key body mismatch");
    check!(get(&c, "pinm.txt").await? == b"first", "existing key overwritten");
    Ok(())
}

async fn delete_object(c: C) -> Result<()> {
    put(&c, "del.txt", b"x").await?;
    c.s.delete(&p("del.txt")?).await.map_err(oerr)?;
    match c.s.head(&p("del.txt")?).await {
        Err(object_store::Error::NotFound { .. }) => Ok(()),
        Err(e) => Err(anyhow!("AssertionError: expected NotFound, got {}", oerr(e))),
        Ok(_) => Err(anyhow!("AssertionError: object still exists")),
    }
}

async fn delete_missing_object(c: C) -> Result<()> {
    c.s.delete(&p(&format!("never-existed-{}", rand_hex8()))?).await.map_err(oerr)?;
    Ok(())
}

async fn delete_objects(c: C) -> Result<()> {
    let keys = ["bulk/1", "bulk/2", "bulk/3"];
    for k in keys {
        put(&c, k, k.as_bytes()).await?;
    }
    let paths: Vec<object_store::Result<Path>> = keys.iter().map(|k| Ok(Path::from(*k))).collect();
    let res: Vec<_> = c.s.delete_stream(futures::stream::iter(paths).boxed()).collect().await;
    check!(res.len() == 3, "{} results", res.len());
    for r in res {
        r.map_err(oerr)?;
    }
    let left: Vec<_> = c.s.list(Some(&p("bulk")?)).try_collect().await.map_err(oerr)?;
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
    let got: Vec<(String, u64, String)> = c
        .s
        .list(Some(&p("lb")?))
        .map_ok(|m| (m.location.to_string(), m.size, unquote(m.e_tag.as_deref().unwrap_or_default())))
        .try_collect()
        .await
        .map_err(oerr)?;
    check!(got == want, "got {got:?}, want {want:?}");
    Ok(())
}

async fn list_v2_prefix_delimiter(c: C) -> Result<()> {
    let b = format!("{}-d", c.b);
    aws_create_bucket(&c.admin, &b).await?;
    let res = async {
        let s = store(&c.env, &b, &c.env.sk)?;
        for k in ["a/1", "a/2", "b/1", "c"] {
            s.put(&p(k)?, PutPayload::from_static(b"x")).await.map_err(oerr)?;
        }
        let r = s.list_with_delimiter(None).await.map_err(oerr)?;
        // object_store strips the trailing delimiter from common prefixes
        let cps: Vec<String> = r.common_prefixes.iter().map(|p| format!("{p}/")).collect();
        let keys: Vec<String> = r.objects.iter().map(|m| m.location.to_string()).collect();
        check!(cps == ["a/", "b/"], "CommonPrefixes {cps:?}");
        check!(keys == ["c"], "Contents {keys:?}");
        Ok(())
    }
    .await;
    aws_destroy_bucket(&c.admin, &b).await;
    res
}

async fn list_v2_pagination(c: C) -> Result<()> {
    let keys: Vec<String> = (0..25).map(|i| format!("pg/k{i:02}")).collect();
    futures::future::try_join_all(keys.iter().map(|k| put(&c, k, b"p"))).await?;
    let mut pages = 0;
    let mut got = vec![];
    let mut token = None;
    loop {
        let r = c
            .s
            .list_paginated(Some("pg/"), PaginatedListOptions { max_keys: Some(10), page_token: token, ..Default::default() })
            .await
            .map_err(oerr)?;
        pages += 1;
        check!(r.result.objects.len() <= 10, "page {pages} has {} keys", r.result.objects.len());
        got.extend(r.result.objects.iter().map(|m| m.location.to_string()));
        token = r.page_token;
        if token.is_none() || pages > 10 {
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
    let keys: Vec<String> = c
        .s
        .list_with_offset(Some(&p("sa")?), &p("sa/k2")?)
        .map_ok(|m| m.location.to_string())
        .try_collect()
        .await
        .map_err(oerr)?;
    check!(keys == ["sa/k3", "sa/k4"], "keys {keys:?}");
    Ok(())
}

async fn copy_object(c: C) -> Result<()> {
    let mut a = Attributes::new();
    a.insert(Attribute::ContentType, "text/x-copy".into());
    a.insert(Attribute::Metadata("color".into()), "Blue".into());
    c.s.put_opts(&p("copy-src")?, PutPayload::from_static(b"copy me"), PutOptions { attributes: a, ..Default::default() })
        .await
        .map_err(oerr)?;
    c.s.copy(&p("copy-src")?, &p("copy-dst")?).await.map_err(oerr)?;
    let r = c.s.get(&p("copy-dst")?).await.map_err(oerr)?;
    let ct = r.attributes.get(&Attribute::ContentType).map(|v| v.to_string());
    let color = r.attributes.get(&Attribute::Metadata("color".into())).map(|v| v.to_string());
    check!(ct.as_deref() == Some("text/x-copy"), "Content-Type {ct:?}");
    check!(color.as_deref() == Some("Blue"), "metadata color {color:?}");
    check!(&r.bytes().await.map_err(oerr)?[..] == b"copy me", "body mismatch");
    Ok(())
}

async fn multipart_upload(c: C) -> Result<()> {
    let parts = vec![rand_bytes(5 << 20), rand_bytes(5 << 20), rand_bytes(1024)];
    let path = p("mpu.bin")?;
    let id = c.s.create_multipart(&path).await.map_err(oerr)?;
    let mut ids = vec![];
    for (i, d) in parts.iter().enumerate() {
        ids.push(c.s.put_part(&path, &id, i, PutPayload::from(d.clone())).await.map_err(oerr)?);
    }
    let r = c.s.complete_multipart(&path, &id, ids).await.map_err(oerr)?;
    let etag = r.e_tag.unwrap_or_default();
    check!(is_multipart_etag(&etag, 3), "ETag {etag} not <hex>-3");
    check!(get(&c, "mpu.bin").await? == parts.concat(), "body mismatch");
    Ok(())
}

async fn multipart_too_small(c: C) -> Result<()> {
    let path = p("mpu-small.bin")?;
    let id = c.s.create_multipart(&path).await.map_err(oerr)?;
    let mut ids = vec![];
    for i in 0..2 {
        ids.push(c.s.put_part(&path, &id, i, PutPayload::from(rand_bytes(1024))).await.map_err(oerr)?);
    }
    let r = c.s.complete_multipart(&path, &id, ids).await;
    let _ = c.s.abort_multipart(&path, &id).await;
    match r {
        Ok(_) => Err(anyhow!("AssertionError: expected EntityTooSmall, got success")),
        Err(e) => {
            check!(e.to_string().contains("EntityTooSmall"), "expected EntityTooSmall, got {}", oerr(e));
            Ok(())
        }
    }
}

async fn high_level_upload(c: C) -> Result<()> {
    let data = rand_bytes(20 << 20);
    let dynstore: Arc<dyn ObjectStore> = c.s.clone();
    let mut w = object_store::buffered::BufWriter::new(dynstore, p("hl.bin")?);
    w.write_all(&data).await?;
    w.shutdown().await?;
    check!(get(&c, "hl.bin").await? == data, "20MiB body mismatch");
    Ok(())
}

async fn presigned_get(c: C) -> Result<()> {
    put(&c, "presign-get.txt", b"presigned get").await?;
    let url = c.s.signed_url(object_store::signer::Method::GET, &p("presign-get.txt")?, Duration::from_secs(600)).await.map_err(oerr)?;
    let r = http().get(url).send().await?;
    let st = r.status();
    let body = r.bytes().await?;
    check!(st.as_u16() == 200, "HTTP {st}: {}", String::from_utf8_lossy(&body));
    check!(&body[..] == b"presigned get", "body mismatch");
    Ok(())
}

async fn presigned_put(c: C) -> Result<()> {
    let url = c.s.signed_url(object_store::signer::Method::PUT, &p("presign-put.txt")?, Duration::from_secs(600)).await.map_err(oerr)?;
    let r = http().put(url).body("presigned put").send().await?;
    let st = r.status();
    check!(st.is_success(), "HTTP {st}: {}", r.text().await.unwrap_or_default());
    check!(get(&c, "presign-put.txt").await? == b"presigned put", "body mismatch");
    Ok(())
}

async fn bad_credentials(c: C) -> Result<()> {
    // object_store has no ListBuckets; use ListObjectsV2 on the bucket instead
    let s = store(&c.env, &c.b, "definitely-wrong-secret")?;
    let r: object_store::Result<Vec<_>> = s.list(None).try_collect().await;
    match r {
        Err(object_store::Error::PermissionDenied { .. }) | Err(object_store::Error::Unauthenticated { .. }) => Ok(()),
        // object_store maps 403 on list requests to Error::Generic; check the reported status
        Err(e) if e.to_string().contains("403 Forbidden") => Ok(()),
        Err(e) => Err(anyhow!("AssertionError: expected 403, got {}", oerr(e))),
        Ok(_) => Err(anyhow!("AssertionError: expected 403, got success")),
    }
}

pub async fn run(env: &Env) -> Report {
    let b = bucket_name("object-store");
    let admin = aws_client(env, &env.sk);
    let mut r = Report::new(SDK, &crate::versions::lock_version("object_store"));
    let setup = aws_create_bucket(&admin, &b).await;
    let s = Arc::new(store(env, &b, &env.sk).expect("build store"));
    let c = Arc::new(Ctx { env: env.clone(), s, b: b.clone(), admin: admin.clone() });
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
    if let Err(e) = setup {
        eprintln!("bucket setup via aws-sdk-s3 failed: {e:#}");
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
    sk!(list_v1, "object_store only implements ListObjectsV2");
    sc!(copy_object);
    sk!(copy_object_replace_metadata, "object_store copy has no metadata directive / replacement metadata");
    sc!(multipart_upload);
    sk!(multipart_abort, "object_store has no ListParts API to verify the abort");
    sk!(multipart_list_parts, "object_store has no ListParts API");
    sc!(multipart_too_small);
    sc!(high_level_upload);
    sc!(presigned_get);
    sc!(presigned_put);
    for s in ["checksum_crc32", "checksum_crc32c", "checksum_sha1", "checksum_sha256", "checksum_crc64nvme"] {
        r.run(s, async { Err(skip("object_store cannot request ChecksumMode or read response checksums on GET")) }).await;
    }
    sk!(content_md5_bad, "object_store has no Content-MD5 option");
    sc!(bad_credentials);
    sk!(delete_bucket_not_empty, NO_BUCKET_API);
    sk!(versioning, "object_store cannot enable bucket versioning or list object versions");
    sk!(object_tagging, "object_store has no GetObjectTagging API");
    sk!(delete_bucket, NO_BUCKET_API);
    aws_destroy_bucket(&admin, &b).await;
    r
}
