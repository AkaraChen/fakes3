use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use aws_sdk_s3::presigning::PresigningConfig;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{
    BucketVersioningStatus, ChecksumAlgorithm, ChecksumMode, CompletedMultipartUpload, CompletedPart, Delete,
    MetadataDirective, ObjectIdentifier, Tag, Tagging, VersioningConfiguration,
};
use aws_sdk_s3::Client;
use base64::Engine;

use crate::check;
use crate::common::*;

pub const SDK: &str = "aws-sdk-s3";

pub struct Ctx {
    pub env: Env,
    pub c: Client,
    pub b: String,
}
type C = Arc<Ctx>;

async fn put(c: &C, key: &str, data: &[u8]) -> Result<String> {
    let r = c
        .c
        .put_object()
        .bucket(&c.b)
        .key(key)
        .body(ByteStream::from(data.to_vec()))
        .send()
        .await
        .map_err(aerr)?;
    Ok(unquote(r.e_tag().unwrap_or_default()))
}

async fn get(c: &C, key: &str) -> Result<Vec<u8>> {
    let r = c.c.get_object().bucket(&c.b).key(key).send().await.map_err(aerr)?;
    Ok(r.body.collect().await?.into_bytes().to_vec())
}

async fn list_all(c: &Client, b: &str, prefix: &str) -> Result<Vec<aws_sdk_s3::types::Object>> {
    let mut out = vec![];
    let mut token = None;
    loop {
        let r = c
            .list_objects_v2()
            .bucket(b)
            .prefix(prefix)
            .set_continuation_token(token)
            .send()
            .await
            .map_err(aerr)?;
        out.extend(r.contents().iter().cloned());
        token = r.next_continuation_token().map(String::from);
        if token.is_none() {
            break;
        }
    }
    Ok(out)
}

async fn create_bucket(c: C) -> Result<()> {
    c.c.create_bucket().bucket(&c.b).send().await.map_err(aerr)?;
    Ok(())
}

async fn head_bucket(c: C) -> Result<()> {
    c.c.head_bucket().bucket(&c.b).send().await.map_err(aerr)?;
    Ok(())
}

async fn head_bucket_missing(c: C) -> Result<()> {
    let r = c.c.head_bucket().bucket(format!("m-missing-{}", rand_hex8())).send().await;
    expect_aws_err(r, &[404], &["NotFound", "NoSuchBucket"])
}

async fn list_buckets(c: C) -> Result<()> {
    let r = c.c.list_buckets().send().await.map_err(aerr)?;
    let names: Vec<_> = r.buckets().iter().filter_map(|b| b.name()).collect();
    check!(names.contains(&c.b.as_str()), "bucket {} not in {:?}", c.b, names);
    Ok(())
}

async fn put_get_small(c: C) -> Result<()> {
    let data = b"hello world";
    let etag = put(&c, "small.txt", data).await?;
    check!(etag == md5_hex(data), "put ETag {etag} != md5 {}", md5_hex(data));
    let r = c.c.get_object().bucket(&c.b).key("small.txt").send().await.map_err(aerr)?;
    let getag = unquote(r.e_tag().unwrap_or_default());
    let body = r.body.collect().await?.into_bytes();
    check!(&body[..] == data, "body mismatch");
    check!(getag == md5_hex(data), "get ETag {getag} != md5");
    Ok(())
}

async fn put_get_empty(c: C) -> Result<()> {
    put(&c, "empty", b"").await?;
    let r = c.c.get_object().bucket(&c.b).key("empty").send().await.map_err(aerr)?;
    check!(r.content_length() == Some(0), "Content-Length {:?}", r.content_length());
    let body = r.body.collect().await?.into_bytes();
    check!(body.is_empty(), "body len {}", body.len());
    Ok(())
}

async fn put_get_large(c: C) -> Result<()> {
    let data = rand_bytes(8 << 20);
    put(&c, "large.bin", &data).await?;
    let got = get(&c, "large.bin").await?;
    check!(got == data, "8MiB body mismatch (got {} bytes)", got.len());
    Ok(())
}

async fn special_keys(c: C) -> Result<()> {
    for (i, k) in SPECIAL_KEYS.iter().enumerate() {
        let data = format!("special-{i}");
        put(&c, k, data.as_bytes()).await?;
        let got = get(&c, k).await?;
        check!(got == data.as_bytes(), "body mismatch for key {k:?}");
    }
    let keys: Vec<String> = list_all(&c.c, &c.b, "").await?.iter().filter_map(|o| o.key().map(String::from)).collect();
    for k in SPECIAL_KEYS {
        check!(keys.iter().any(|x| x == k), "key {k:?} not in listing");
    }
    Ok(())
}

async fn head_object_metadata(c: C) -> Result<()> {
    c.c.put_object()
        .bucket(&c.b)
        .key("meta.txt")
        .body(ByteStream::from_static(b"metadata"))
        .content_type("text/x-matrix")
        .cache_control("max-age=3600")
        .content_disposition("attachment; filename=\"m.txt\"")
        .content_encoding("gzip")
        .metadata("color", "Blue")
        .metadata("x-y", "z")
        .send()
        .await
        .map_err(aerr)?;
    let h = c.c.head_object().bucket(&c.b).key("meta.txt").send().await.map_err(aerr)?;
    check!(h.content_type() == Some("text/x-matrix"), "Content-Type {:?}", h.content_type());
    check!(h.cache_control() == Some("max-age=3600"), "Cache-Control {:?}", h.cache_control());
    check!(
        h.content_disposition() == Some("attachment; filename=\"m.txt\""),
        "Content-Disposition {:?}",
        h.content_disposition()
    );
    check!(h.content_encoding() == Some("gzip"), "Content-Encoding {:?}", h.content_encoding());
    let want: HashMap<String, String> = [("color", "Blue"), ("x-y", "z")].iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
    check!(h.metadata() == Some(&want), "metadata {:?}", h.metadata());
    Ok(())
}

async fn get_range(c: C) -> Result<()> {
    let data = range_body();
    put(&c, "range.bin", &data).await?;
    for (range, lo, hi) in [("bytes=10-19", 10, 19), ("bytes=-5", 95, 99), ("bytes=95-", 95, 99)] {
        let r = c.c.get_object().bucket(&c.b).key("range.bin").range(range).send().await.map_err(aerr)?;
        let cr = r.content_range().map(String::from);
        let body = r.body.collect().await?.into_bytes();
        check!(&body[..] == &data[lo..=hi], "{range}: body mismatch ({} bytes)", body.len());
        let want = format!("bytes {lo}-{hi}/100");
        check!(cr.as_deref() == Some(want.as_str()), "{range}: Content-Range {cr:?} != {want}");
    }
    Ok(())
}

async fn get_range_invalid(c: C) -> Result<()> {
    put(&c, "range-inv.bin", &range_body()).await?;
    let r = c.c.get_object().bucket(&c.b).key("range-inv.bin").range("bytes=200-300").send().await;
    expect_aws_err(r, &[416], &["InvalidRange"])
}

async fn get_missing_key(c: C) -> Result<()> {
    let r = c.c.get_object().bucket(&c.b).key("does/not/exist").send().await;
    expect_aws_err(r, &[404], &["NoSuchKey"])
}

async fn get_if_none_match(c: C) -> Result<()> {
    let etag = put(&c, "inm.txt", b"if-none-match").await?;
    let r = c.c.get_object().bucket(&c.b).key("inm.txt").if_none_match(format!("\"{etag}\"")).send().await;
    expect_aws_err(r, &[304], &[])
}

async fn get_if_match_fail(c: C) -> Result<()> {
    put(&c, "im.txt", b"if-match").await?;
    let r = c.c.get_object().bucket(&c.b).key("im.txt").if_match("\"wrong\"").send().await;
    expect_aws_err(r, &[412], &[])
}

async fn put_if_none_match(c: C) -> Result<()> {
    put(&c, "pinm.txt", b"first").await?;
    let r = c
        .c
        .put_object()
        .bucket(&c.b)
        .key("pinm.txt")
        .if_none_match("*")
        .body(ByteStream::from_static(b"second"))
        .send()
        .await;
    expect_aws_err(r, &[412], &[])?;
    let k = format!("pinm-new-{}", rand_hex8());
    c.c.put_object()
        .bucket(&c.b)
        .key(&k)
        .if_none_match("*")
        .body(ByteStream::from_static(b"new"))
        .send()
        .await
        .map_err(aerr)?;
    check!(get(&c, &k).await? == b"new", "new key body mismatch");
    check!(get(&c, "pinm.txt").await? == b"first", "existing key was overwritten");
    Ok(())
}

async fn delete_object(c: C) -> Result<()> {
    put(&c, "del.txt", b"x").await?;
    c.c.delete_object().bucket(&c.b).key("del.txt").send().await.map_err(aerr)?;
    let r = c.c.head_object().bucket(&c.b).key("del.txt").send().await;
    expect_aws_err(r, &[404], &["NotFound", "NoSuchKey"])
}

async fn delete_missing_object(c: C) -> Result<()> {
    c.c.delete_object().bucket(&c.b).key(format!("never-existed-{}", rand_hex8())).send().await.map_err(aerr)?;
    Ok(())
}

async fn delete_objects(c: C) -> Result<()> {
    let keys = ["bulk/1", "bulk/2", "bulk/3"];
    for k in keys {
        put(&c, k, k.as_bytes()).await?;
    }
    let objs = keys.iter().map(|k| ObjectIdentifier::builder().key(*k).build()).collect::<Result<Vec<_>, _>>()?;
    let r = c
        .c
        .delete_objects()
        .bucket(&c.b)
        .delete(Delete::builder().set_objects(Some(objs)).build()?)
        .send()
        .await
        .map_err(aerr)?;
    check!(r.errors().is_empty(), "errors: {:?}", r.errors());
    check!(r.deleted().len() == 3, "deleted {} entries", r.deleted().len());
    let left = list_all(&c.c, &c.b, "bulk/").await?;
    check!(left.is_empty(), "{} keys remain", left.len());
    for k in keys {
        expect_aws_err(c.c.head_object().bucket(&c.b).key(k).send().await, &[404], &["NotFound"])?;
    }
    Ok(())
}

async fn list_v2_basic(c: C) -> Result<()> {
    let mut want = vec![];
    for (i, k) in ["lb/c", "lb/a", "lb/b"].iter().enumerate() {
        let data = vec![b'x'; i * 7 + 1];
        put(&c, k, &data).await?;
        want.push((k.to_string(), data.len() as i64, md5_hex(&data)));
    }
    want.sort();
    let r = c.c.list_objects_v2().bucket(&c.b).prefix("lb/").send().await.map_err(aerr)?;
    let got: Vec<(String, i64, String)> = r
        .contents()
        .iter()
        .map(|o| (o.key().unwrap_or_default().to_string(), o.size().unwrap_or(-1), unquote(o.e_tag().unwrap_or_default())))
        .collect();
    check!(got == want, "got {got:?}, want {want:?}");
    check!(r.key_count() == Some(3), "KeyCount {:?}", r.key_count());
    Ok(())
}

async fn list_v2_prefix_delimiter(c: C) -> Result<()> {
    // dedicated bucket so that prefix="" lists only the scenario's keys
    let b = format!("{}-d", c.b);
    aws_create_bucket(&c.c, &b).await?;
    let res = async {
        for k in ["a/1", "a/2", "b/1", "c"] {
            c.c.put_object().bucket(&b).key(k).body(ByteStream::from_static(b"x")).send().await.map_err(aerr)?;
        }
        let r = c.c.list_objects_v2().bucket(&b).prefix("").delimiter("/").send().await.map_err(aerr)?;
        let cps: Vec<_> = r.common_prefixes().iter().filter_map(|p| p.prefix()).collect();
        let keys: Vec<_> = r.contents().iter().filter_map(|o| o.key()).collect();
        check!(cps == ["a/", "b/"], "CommonPrefixes {cps:?}");
        check!(keys == ["c"], "Contents {keys:?}");
        Ok(())
    }
    .await;
    aws_destroy_bucket(&c.c, &b).await;
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
            .c
            .list_objects_v2()
            .bucket(&c.b)
            .prefix("pg/")
            .max_keys(10)
            .set_continuation_token(token)
            .send()
            .await
            .map_err(aerr)?;
        pages += 1;
        check!(r.contents().len() <= 10, "page {pages} has {} keys", r.contents().len());
        got.extend(r.contents().iter().filter_map(|o| o.key().map(String::from)));
        token = r.next_continuation_token().map(String::from);
        if !r.is_truncated().unwrap_or(false) || token.is_none() || pages > 10 {
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
    let r = c.c.list_objects_v2().bucket(&c.b).prefix("sa/").start_after("sa/k2").send().await.map_err(aerr)?;
    let keys: Vec<_> = r.contents().iter().filter_map(|o| o.key()).collect();
    check!(keys == ["sa/k3", "sa/k4"], "keys {keys:?}");
    Ok(())
}

async fn list_v1(c: C) -> Result<()> {
    for i in 0..5 {
        put(&c, &format!("v1/k{i}"), b"v").await?;
    }
    let r = c.c.list_objects().bucket(&c.b).prefix("v1/").marker("v1/k1").send().await.map_err(aerr)?;
    let keys: Vec<_> = r.contents().iter().filter_map(|o| o.key()).collect();
    check!(keys == ["v1/k2", "v1/k3", "v1/k4"], "keys {keys:?}");
    // paged v1 listing with MaxKeys + Marker
    let r = c.c.list_objects().bucket(&c.b).prefix("v1/").max_keys(2).send().await.map_err(aerr)?;
    let keys: Vec<_> = r.contents().iter().filter_map(|o| o.key()).collect();
    check!(keys == ["v1/k0", "v1/k1"], "page1 {keys:?}");
    check!(r.is_truncated() == Some(true), "IsTruncated {:?}", r.is_truncated());
    Ok(())
}

async fn copy_object(c: C) -> Result<()> {
    c.c.put_object()
        .bucket(&c.b)
        .key("copy-src")
        .body(ByteStream::from_static(b"copy me"))
        .content_type("text/x-copy")
        .metadata("color", "Blue")
        .send()
        .await
        .map_err(aerr)?;
    c.c.copy_object().bucket(&c.b).key("copy-dst").copy_source(format!("{}/copy-src", c.b)).send().await.map_err(aerr)?;
    let r = c.c.get_object().bucket(&c.b).key("copy-dst").send().await.map_err(aerr)?;
    check!(r.content_type() == Some("text/x-copy"), "Content-Type {:?}", r.content_type());
    check!(
        r.metadata().and_then(|m| m.get("color")).map(String::as_str) == Some("Blue"),
        "metadata {:?}",
        r.metadata()
    );
    let body = r.body.collect().await?.into_bytes();
    check!(&body[..] == b"copy me", "body mismatch");
    Ok(())
}

async fn copy_object_replace_metadata(c: C) -> Result<()> {
    c.c.put_object()
        .bucket(&c.b)
        .key("crm-src")
        .body(ByteStream::from_static(b"replace"))
        .content_type("text/x-old")
        .metadata("old", "1")
        .send()
        .await
        .map_err(aerr)?;
    c.c.copy_object()
        .bucket(&c.b)
        .key("crm-dst")
        .copy_source(format!("{}/crm-src", c.b))
        .metadata_directive(MetadataDirective::Replace)
        .content_type("text/x-new")
        .metadata("new", "2")
        .send()
        .await
        .map_err(aerr)?;
    let h = c.c.head_object().bucket(&c.b).key("crm-dst").send().await.map_err(aerr)?;
    let want: HashMap<String, String> = [("new".to_string(), "2".to_string())].into();
    check!(h.metadata() == Some(&want), "metadata {:?}", h.metadata());
    check!(h.content_type() == Some("text/x-new"), "Content-Type {:?}", h.content_type());
    check!(get(&c, "crm-dst").await? == b"replace", "body mismatch");
    Ok(())
}

async fn mpu_upload_parts(c: &C, key: &str, parts: &[Vec<u8>]) -> Result<(String, Vec<CompletedPart>)> {
    let up = c.c.create_multipart_upload().bucket(&c.b).key(key).send().await.map_err(aerr)?;
    let id = up.upload_id().unwrap_or_default().to_string();
    let mut done = vec![];
    for (i, p) in parts.iter().enumerate() {
        let n = i as i32 + 1;
        let r = c
            .c
            .upload_part()
            .bucket(&c.b)
            .key(key)
            .upload_id(&id)
            .part_number(n)
            .body(ByteStream::from(p.clone()))
            .send()
            .await
            .map_err(aerr)?;
        done.push(CompletedPart::builder().part_number(n).set_e_tag(r.e_tag().map(String::from)).build());
    }
    Ok((id, done))
}

async fn multipart_upload(c: C) -> Result<()> {
    let parts = vec![rand_bytes(5 << 20), rand_bytes(5 << 20), rand_bytes(1024)];
    let (id, done) = mpu_upload_parts(&c, "mpu.bin", &parts).await?;
    let r = c
        .c
        .complete_multipart_upload()
        .bucket(&c.b)
        .key("mpu.bin")
        .upload_id(id)
        .multipart_upload(CompletedMultipartUpload::builder().set_parts(Some(done)).build())
        .send()
        .await
        .map_err(aerr)?;
    let etag = r.e_tag().unwrap_or_default().to_string();
    check!(is_multipart_etag(&etag, 3), "ETag {etag} not <hex>-3");
    check!(get(&c, "mpu.bin").await? == parts.concat(), "body mismatch");
    Ok(())
}

async fn multipart_abort(c: C) -> Result<()> {
    let (id, _) = mpu_upload_parts(&c, "mpu-abort.bin", &[rand_bytes(1024)]).await?;
    c.c.abort_multipart_upload().bucket(&c.b).key("mpu-abort.bin").upload_id(&id).send().await.map_err(aerr)?;
    let r = c.c.list_parts().bucket(&c.b).key("mpu-abort.bin").upload_id(&id).send().await;
    expect_aws_err(r, &[], &["NoSuchUpload"])
}

async fn multipart_list_parts(c: C) -> Result<()> {
    let parts = vec![rand_bytes(5 << 20), rand_bytes(1024)];
    let (id, _) = mpu_upload_parts(&c, "mpu-lp.bin", &parts).await?;
    let r = c.c.list_parts().bucket(&c.b).key("mpu-lp.bin").upload_id(&id).send().await.map_err(aerr);
    let _ = c.c.abort_multipart_upload().bucket(&c.b).key("mpu-lp.bin").upload_id(&id).send().await;
    let r = r?;
    let got: Vec<(i32, i64, String)> = r
        .parts()
        .iter()
        .map(|p| (p.part_number().unwrap_or(0), p.size().unwrap_or(-1), unquote(p.e_tag().unwrap_or_default())))
        .collect();
    let want: Vec<(i32, i64, String)> =
        parts.iter().enumerate().map(|(i, p)| (i as i32 + 1, p.len() as i64, md5_hex(p))).collect();
    check!(got == want, "parts {got:?} != {want:?}");
    Ok(())
}

async fn multipart_too_small(c: C) -> Result<()> {
    let (id, done) = mpu_upload_parts(&c, "mpu-small.bin", &[rand_bytes(1024), rand_bytes(1024)]).await?;
    let r = c
        .c
        .complete_multipart_upload()
        .bucket(&c.b)
        .key("mpu-small.bin")
        .upload_id(&id)
        .multipart_upload(CompletedMultipartUpload::builder().set_parts(Some(done)).build())
        .send()
        .await;
    let _ = c.c.abort_multipart_upload().bucket(&c.b).key("mpu-small.bin").upload_id(&id).send().await;
    expect_aws_err(r, &[], &["EntityTooSmall"])
}

async fn high_level_upload(c: C) -> Result<()> {
    let data = rand_bytes(20 << 20);
    let tm = aws_sdk_s3_transfer_manager::Client::new(
        aws_sdk_s3_transfer_manager::Config::builder().client(c.c.clone()).build(),
    );
    let h = tm
        .upload()
        .bucket(&c.b)
        .key("hl.bin")
        .body(aws_sdk_s3_transfer_manager::io::InputStream::from(bytes::Bytes::from(data.clone())))
        .initiate()?;
    h.join().await.map_err(|e| anyhow::anyhow!("TransferManagerError: {}", aws_sdk_s3::error::DisplayErrorContext(&e)))?;
    check!(get(&c, "hl.bin").await? == data, "20MiB body mismatch");
    Ok(())
}

async fn presigned_get(c: C) -> Result<()> {
    put(&c, "presign-get.txt", b"presigned get").await?;
    let p = c
        .c
        .get_object()
        .bucket(&c.b)
        .key("presign-get.txt")
        .presigned(PresigningConfig::expires_in(Duration::from_secs(600))?)
        .await
        .map_err(aerr)?;
    let r = http().get(p.uri()).send().await?;
    let st = r.status();
    let body = r.bytes().await?;
    check!(st.as_u16() == 200, "HTTP {st}: {}", String::from_utf8_lossy(&body));
    check!(&body[..] == b"presigned get", "body mismatch");
    Ok(())
}

async fn presigned_put(c: C) -> Result<()> {
    let p = c
        .c
        .put_object()
        .bucket(&c.b)
        .key("presign-put.txt")
        .presigned(PresigningConfig::expires_in(Duration::from_secs(600))?)
        .await
        .map_err(aerr)?;
    let mut req = http().put(p.uri()).body("presigned put");
    for (k, v) in p.headers() {
        req = req.header(k, v);
    }
    let r = req.send().await?;
    let st = r.status();
    check!(st.is_success(), "HTTP {st}: {}", r.text().await.unwrap_or_default());
    check!(get(&c, "presign-put.txt").await? == b"presigned put", "body mismatch");
    Ok(())
}

fn expected_checksum(alg: &ChecksumAlgorithm, data: &[u8]) -> String {
    use sha1::Digest as _;
    let b = base64::engine::general_purpose::STANDARD;
    match alg {
        ChecksumAlgorithm::Crc32 => b.encode(crc32fast::hash(data).to_be_bytes()),
        ChecksumAlgorithm::Crc32C => b.encode(crc32c::crc32c(data).to_be_bytes()),
        ChecksumAlgorithm::Sha1 => b.encode(sha1::Sha1::digest(data)),
        ChecksumAlgorithm::Sha256 => b.encode(sha2::Sha256::digest(data)),
        ChecksumAlgorithm::Crc64Nvme => {
            let mut d = crc64fast_nvme::Digest::new();
            d.write(data);
            b.encode(d.sum64().to_be_bytes())
        }
        _ => unreachable!(),
    }
}

async fn checksum(c: C, alg: ChecksumAlgorithm) -> Result<()> {
    let data = rand_bytes(64 * 1024 + 7);
    let key = format!("cksum-{}", alg.as_str());
    c.c.put_object()
        .bucket(&c.b)
        .key(&key)
        .checksum_algorithm(alg.clone())
        .body(ByteStream::from(data.clone()))
        .send()
        .await
        .map_err(aerr)?;
    let r = c.c.get_object().bucket(&c.b).key(&key).checksum_mode(ChecksumMode::Enabled).send().await.map_err(aerr)?;
    let got = match alg {
        ChecksumAlgorithm::Crc32 => r.checksum_crc32(),
        ChecksumAlgorithm::Crc32C => r.checksum_crc32_c(),
        ChecksumAlgorithm::Sha1 => r.checksum_sha1(),
        ChecksumAlgorithm::Sha256 => r.checksum_sha256(),
        ChecksumAlgorithm::Crc64Nvme => r.checksum_crc64_nvme(),
        _ => None,
    }
    .map(String::from);
    let body = r.body.collect().await?.into_bytes();
    check!(body[..] == data[..], "body mismatch");
    let want = expected_checksum(&alg, &data);
    check!(got.as_deref() == Some(want.as_str()), "checksum {:?} != {want}", got);
    Ok(())
}

async fn content_md5_bad(c: C) -> Result<()> {
    let wrong = base64::engine::general_purpose::STANDARD.encode(<md5::Md5 as md5::Digest>::digest(b"something else"));
    let r = c
        .c
        .put_object()
        .bucket(&c.b)
        .key("badmd5")
        .content_md5(wrong)
        .body(ByteStream::from_static(b"actual body"))
        .send()
        .await;
    expect_aws_err(r, &[400], &["BadDigest", "InvalidDigest"])
}

async fn bad_credentials(c: C) -> Result<()> {
    let bad = aws_client(&c.env, "definitely-wrong-secret");
    let r = bad.list_buckets().send().await;
    expect_aws_err(r, &[403], &["SignatureDoesNotMatch", "InvalidAccessKeyId"])
}

async fn delete_bucket_not_empty(c: C) -> Result<()> {
    put(&c, "keep-bucket-nonempty", b"x").await?;
    let r = c.c.delete_bucket().bucket(&c.b).send().await;
    expect_aws_err(r, &[409], &["BucketNotEmpty"])
}

async fn versioning(c: C) -> Result<()> {
    let b = format!("{}-v", c.b);
    aws_create_bucket(&c.c, &b).await?;
    let res = async {
        c.c.put_bucket_versioning()
            .bucket(&b)
            .versioning_configuration(VersioningConfiguration::builder().status(BucketVersioningStatus::Enabled).build())
            .send()
            .await
            .map_err(aerr)?;
        let v1 = c.c.put_object().bucket(&b).key("ver").body(ByteStream::from_static(b"version-1")).send().await.map_err(aerr)?;
        let v2 = c.c.put_object().bucket(&b).key("ver").body(ByteStream::from_static(b"version-2")).send().await.map_err(aerr)?;
        let id1 = v1.version_id().unwrap_or_default().to_string();
        let id2 = v2.version_id().unwrap_or_default().to_string();
        check!(!id1.is_empty() && id1 != "null" && id1 != id2, "version ids {id1:?} {id2:?}");
        let l = c.c.list_object_versions().bucket(&b).prefix("ver").send().await.map_err(aerr)?;
        let vs: Vec<_> = l.versions().iter().filter(|v| v.key() == Some("ver")).collect();
        check!(vs.len() == 2, "{} versions listed", vs.len());
        let ids: Vec<_> = vs.iter().filter_map(|v| v.version_id()).collect();
        check!(ids.contains(&id1.as_str()) && ids.contains(&id2.as_str()), "listed ids {ids:?}");
        let old = c.c.get_object().bucket(&b).key("ver").version_id(&id1).send().await.map_err(aerr)?;
        let body = old.body.collect().await?.into_bytes();
        check!(&body[..] == b"version-1", "old version body {:?}", String::from_utf8_lossy(&body));
        let cur = c.c.get_object().bucket(&b).key("ver").send().await.map_err(aerr)?;
        let body = cur.body.collect().await?.into_bytes();
        check!(&body[..] == b"version-2", "current body mismatch");
        Ok(())
    }
    .await;
    aws_destroy_bucket(&c.c, &b).await;
    res
}

async fn object_tagging(c: C) -> Result<()> {
    put(&c, "tagged", b"t").await?;
    let tags = vec![Tag::builder().key("env").value("test").build()?, Tag::builder().key("team").value("s3 matrix").build()?];
    c.c.put_object_tagging()
        .bucket(&c.b)
        .key("tagged")
        .tagging(Tagging::builder().set_tag_set(Some(tags)).build()?)
        .send()
        .await
        .map_err(aerr)?;
    let r = c.c.get_object_tagging().bucket(&c.b).key("tagged").send().await.map_err(aerr)?;
    let mut got: Vec<(String, String)> = r.tag_set().iter().map(|t| (t.key().to_string(), t.value().to_string())).collect();
    got.sort();
    let want = vec![("env".to_string(), "test".to_string()), ("team".to_string(), "s3 matrix".to_string())];
    check!(got == want, "tags {got:?}");
    Ok(())
}

async fn delete_bucket(c: C) -> Result<()> {
    aws_empty_bucket(&c.c, &c.b).await?;
    c.c.delete_bucket().bucket(&c.b).send().await.map_err(aerr)?;
    let r = c.c.head_bucket().bucket(&c.b).send().await;
    expect_aws_err(r, &[404], &["NotFound", "NoSuchBucket"])
}

pub async fn run(env: &Env) -> Report {
    let c = Arc::new(Ctx { env: env.clone(), c: aws_client(env, &env.sk), b: bucket_name("aws-sdk-s3") });
    let mut r = Report::new(SDK, &crate::versions::lock_version("aws-sdk-s3"));
    macro_rules! sc {
        ($name:ident) => {
            r.run(stringify!($name), $name(c.clone())).await
        };
    }
    sc!(create_bucket);
    sc!(head_bucket);
    sc!(head_bucket_missing);
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
    sc!(multipart_abort);
    sc!(multipart_list_parts);
    sc!(multipart_too_small);
    sc!(high_level_upload);
    sc!(presigned_get);
    sc!(presigned_put);
    r.run("checksum_crc32", checksum(c.clone(), ChecksumAlgorithm::Crc32)).await;
    r.run("checksum_crc32c", checksum(c.clone(), ChecksumAlgorithm::Crc32C)).await;
    r.run("checksum_sha1", checksum(c.clone(), ChecksumAlgorithm::Sha1)).await;
    r.run("checksum_sha256", checksum(c.clone(), ChecksumAlgorithm::Sha256)).await;
    r.run("checksum_crc64nvme", checksum(c.clone(), ChecksumAlgorithm::Crc64Nvme)).await;
    sc!(content_md5_bad);
    sc!(bad_credentials);
    sc!(delete_bucket_not_empty);
    sc!(versioning);
    sc!(object_tagging);
    sc!(delete_bucket);
    aws_destroy_bucket(&c.c, &c.b).await;
    r
}
