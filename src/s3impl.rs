use std::collections::BTreeMap;
use std::collections::HashSet;
use std::ops::Bound;
use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use futures::StreamExt;
use md5::{Digest, Md5};
use s3s::checksum::ChecksumHasher;
use s3s::dto::*;
use s3s::{S3, S3Error, S3ErrorCode, S3Request, S3Response, S3Result, s3_error};

use crate::store::{BlobWriter, Bucket as StoreBucket, Entry, Object as StoreObject, ObjectData, ObjectMeta, Part, Store, Upload, Version, Versioning};

pub struct FakeS3 {
    pub store: Store,
    pub inline_threshold: usize,
}

fn io_err(e: std::io::Error) -> S3Error {
    s3_error!(e, InternalError)
}

fn ts(t: SystemTime) -> Timestamp {
    Timestamp::from(t)
}

macro_rules! checksum_from {
    ($src:expr) => {{
        let src = $src;
        Checksum {
            checksum_crc32: src.checksum_crc32.clone(),
            checksum_crc32c: src.checksum_crc32c.clone(),
            checksum_sha1: src.checksum_sha1.clone(),
            checksum_sha256: src.checksum_sha256.clone(),
            checksum_crc64nvme: src.checksum_crc64nvme.clone(),
            checksum_sha512: src.checksum_sha512.clone(),
            checksum_md5: src.checksum_md5.clone(),
            checksum_xxhash64: src.checksum_xxhash64.clone(),
            checksum_xxhash3: src.checksum_xxhash3.clone(),
            checksum_xxhash128: src.checksum_xxhash128.clone(),
            ..Default::default()
        }
    }};
}

/// Copies checksum fields from `$c` into the struct literal under construction.
macro_rules! with_checksum {
    ($ty:ident { $($field:ident : $val:expr),* $(,)? }, $c:expr) => {{
        let c: Checksum = $c;
        $ty {
            $($field: $val,)*
            checksum_crc32: c.checksum_crc32,
            checksum_crc32c: c.checksum_crc32c,
            checksum_sha1: c.checksum_sha1,
            checksum_sha256: c.checksum_sha256,
            checksum_crc64nvme: c.checksum_crc64nvme,
            checksum_sha512: c.checksum_sha512,
            checksum_md5: c.checksum_md5,
            checksum_xxhash64: c.checksum_xxhash64,
            checksum_xxhash3: c.checksum_xxhash3,
            checksum_xxhash128: c.checksum_xxhash128,
            ..Default::default()
        }
    }};
}

const TRAILER_NAMES: &[&str] = &[
    "x-amz-checksum-crc32",
    "x-amz-checksum-crc32c",
    "x-amz-checksum-sha1",
    "x-amz-checksum-sha256",
    "x-amz-checksum-crc64nvme",
    "x-amz-checksum-sha512",
    "x-amz-checksum-md5",
    "x-amz-checksum-xxhash64",
    "x-amz-checksum-xxhash3",
    "x-amz-checksum-xxhash128",
];

fn checksum_slot<'a>(c: &'a mut Checksum, i: usize) -> &'a mut Option<String> {
    match i {
        0 => &mut c.checksum_crc32,
        1 => &mut c.checksum_crc32c,
        2 => &mut c.checksum_sha1,
        3 => &mut c.checksum_sha256,
        4 => &mut c.checksum_crc64nvme,
        5 => &mut c.checksum_sha512,
        6 => &mut c.checksum_md5,
        7 => &mut c.checksum_xxhash64,
        8 => &mut c.checksum_xxhash3,
        _ => &mut c.checksum_xxhash128,
    }
}

fn hasher_for(expected: &Checksum, algorithm: Option<&ChecksumAlgorithm>) -> S3Result<ChecksumHasher> {
    let mut h = ChecksumHasher::default();
    if expected.checksum_crc32.is_some() { h.crc32 = Some(Default::default()); }
    if expected.checksum_crc32c.is_some() { h.crc32c = Some(Default::default()); }
    if expected.checksum_sha1.is_some() { h.sha1 = Some(Default::default()); }
    if expected.checksum_sha256.is_some() { h.sha256 = Some(Default::default()); }
    if expected.checksum_crc64nvme.is_some() { h.crc64nvme = Some(Default::default()); }
    if expected.checksum_sha512.is_some() { h.sha512 = Some(Default::default()); }
    if expected.checksum_md5.is_some() { h.md5 = Some(Default::default()); }
    if expected.checksum_xxhash64.is_some() { h.xxhash64 = Some(Default::default()); }
    if expected.checksum_xxhash3.is_some() { h.xxhash3 = Some(Default::default()); }
    if expected.checksum_xxhash128.is_some() { h.xxhash128 = Some(Default::default()); }
    if let Some(alg) = algorithm {
        match alg.as_str().to_ascii_uppercase().as_str() {
            ChecksumAlgorithm::CRC32 => h.crc32 = Some(Default::default()),
            ChecksumAlgorithm::CRC32C => h.crc32c = Some(Default::default()),
            ChecksumAlgorithm::SHA1 => h.sha1 = Some(Default::default()),
            ChecksumAlgorithm::SHA256 => h.sha256 = Some(Default::default()),
            ChecksumAlgorithm::CRC64NVME => h.crc64nvme = Some(Default::default()),
            ChecksumAlgorithm::SHA512 => h.sha512 = Some(Default::default()),
            ChecksumAlgorithm::MD5 => h.md5 = Some(Default::default()),
            ChecksumAlgorithm::XXHASH64 => h.xxhash64 = Some(Default::default()),
            ChecksumAlgorithm::XXHASH3 => h.xxhash3 = Some(Default::default()),
            ChecksumAlgorithm::XXHASH128 => h.xxhash128 = Some(Default::default()),
            _ => return Err(s3_error!(InvalidArgument, "unsupported checksum algorithm")),
        }
    }
    Ok(h)
}

struct Ingested {
    data: Bytes,
    md5: [u8; 16],
    checksum: Checksum,
}

/// Reads a request body into a blob, validating Content-MD5 and any flexible checksums
/// (header or aws-chunked trailer) the client supplied.
async fn ingest(
    threshold: usize,
    body: Option<StreamingBlob>,
    content_length: Option<i64>,
    content_md5: Option<&str>,
    mut expected: Checksum,
    algorithm: Option<&ChecksumAlgorithm>,
    trailers: Option<s3s::TrailingHeaders>,
) -> S3Result<Ingested> {
    let mut hasher = hasher_for(&expected, algorithm)?;
    let mut md5 = Md5::new();
    let mut w = BlobWriter::new(threshold, content_length.map(|n| n as u64)).map_err(io_err)?;
    if let Some(mut body) = body {
        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(|e| s3_error!(IncompleteBody, "{e}"))?;
            md5.update(&chunk);
            hasher.update(&chunk);
            w.write(&chunk).map_err(io_err)?;
        }
    }
    let data = w.finish().map_err(io_err)?;
    if let Some(n) = content_length {
        if n as usize != data.len() {
            return Err(s3_error!(IncompleteBody));
        }
    }
    let md5: [u8; 16] = md5.finalize().into();

    if let Some(cm) = content_md5 {
        let want = base64_simd::STANDARD.decode_to_vec(cm).map_err(|_| s3_error!(InvalidDigest))?;
        if want != md5 {
            return Err(s3_error!(BadDigest, "The Content-MD5 you specified did not match what we received."));
        }
    }

    if let Some(t) = trailers.and_then(|t| t.take()) {
        for (i, name) in TRAILER_NAMES.iter().enumerate() {
            if let Some(v) = t.get(*name) {
                let v = v.to_str().map_err(|_| s3_error!(InvalidArgument))?.to_owned();
                *checksum_slot(&mut expected, i) = Some(v);
            }
        }
    }

    let actual = hasher.finalize();
    for i in 0..TRAILER_NAMES.len() {
        let mut a = actual.clone();
        let want = checksum_slot(&mut expected, i).clone();
        if let Some(want) = want {
            if checksum_slot(&mut a, i).as_deref() != Some(want.as_str()) {
                return Err(s3_error!(BadDigest, "The {} you specified did not match the calculated checksum.", TRAILER_NAMES[i]));
            }
        }
    }
    Ok(Ingested { data, md5, checksum: actual })
}

fn check_conditions(
    obj: &StoreObject,
    if_match: Option<&ETagCondition>,
    if_none_match: Option<&ETagCondition>,
    if_modified_since: Option<&Timestamp>,
    if_unmodified_since: Option<&Timestamp>,
) -> S3Result<()> {
    let etag = ETag::Strong(obj.e_tag.clone());
    let lm = ts(obj.last_modified);
    let matches = |c: &ETagCondition| c.is_any() || c.as_etag().is_some_and(|e| e.strong_cmp(&etag));
    if let Some(c) = if_match {
        if !matches(c) {
            return Err(s3_error!(PreconditionFailed));
        }
    } else if let Some(t) = if_unmodified_since {
        if lm > *t {
            return Err(s3_error!(PreconditionFailed));
        }
    }
    let not_modified = match if_none_match {
        Some(c) => matches(c),
        None => if_modified_since.is_some_and(|t| lm <= *t),
    };
    if not_modified {
        // RFC 9110 §15.4.5: a 304 carries the validators the 200 would have.
        let mut e = S3Error::new(S3ErrorCode::NotModified);
        let mut h = http::HeaderMap::new();
        if let Ok(v) = etag.to_http_header() {
            h.insert(http::header::ETAG, v);
        }
        let mut buf = Vec::new();
        if lm.format(TimestampFormat::HttpDate, &mut buf).is_ok() {
            if let Ok(v) = http::HeaderValue::from_bytes(&buf) {
                h.insert(http::header::LAST_MODIFIED, v);
            }
        }
        e.set_headers(h);
        return Err(e);
    }
    Ok(())
}

fn validate_bucket_name(name: &str) -> S3Result<()> {
    let ok = (3..=63).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
        && name.as_bytes()[0].is_ascii_alphanumeric()
        && name.as_bytes()[name.len() - 1].is_ascii_alphanumeric()
        && !name.contains("..");
    if ok { Ok(()) } else { Err(s3_error!(InvalidBucketName)) }
}

type Tags = Vec<(String, String)>;

fn parse_tagging_header(h: Option<&str>) -> S3Result<Tags> {
    let Some(h) = h else { return Ok(Vec::new()) };
    let tags: Tags = form_urlencoded::parse(h.as_bytes()).map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
    validate_tags(&tags)?;
    Ok(tags)
}

fn tags_from_set(set: TagSet) -> S3Result<Tags> {
    let tags: Tags = set.into_iter().map(|t| (t.key.unwrap_or_default(), t.value.unwrap_or_default())).collect();
    validate_tags(&tags)?;
    Ok(tags)
}

fn validate_tags(tags: &Tags) -> S3Result<()> {
    if tags.len() > 10 {
        return Err(S3Error::with_message(S3ErrorCode::Custom("BadRequest".into()), "Object tags cannot be greater than 10"));
    }
    let mut seen = HashSet::new();
    for (k, v) in tags {
        if k.is_empty() || k.chars().count() > 128 || v.chars().count() > 256 {
            return Err(s3_error!(InvalidTag, "The TagKey or TagValue you have provided is invalid"));
        }
        if !seen.insert(k) {
            return Err(s3_error!(InvalidTag, "Cannot provide multiple Tags with the same key"));
        }
    }
    Ok(())
}

fn tag_set(tags: &Tags) -> TagSet {
    tags.iter().map(|(k, v)| Tag { key: Some(k.clone()), value: Some(v.clone()) }).collect()
}

fn tag_count(tags: &Tags) -> Option<i32> {
    (!tags.is_empty()).then_some(tags.len() as i32)
}

/// Parses `x-amz-copy-source-range: bytes=first-last` against an object of `size` bytes.
fn parse_copy_range(r: &str, size: u64) -> S3Result<std::ops::Range<u64>> {
    let bad = || s3_error!(InvalidArgument, "The x-amz-copy-source-range value must be of the form bytes=first-last where first and last are the zero-based offsets of the first and last bytes to copy");
    let (a, b) = r.strip_prefix("bytes=").and_then(|x| x.split_once('-')).ok_or_else(bad)?;
    let (a, b): (u64, u64) = (a.parse().map_err(|_| bad())?, b.parse().map_err(|_| bad())?);
    if a > b || b >= size {
        return Err(s3_error!(InvalidRange));
    }
    Ok(a..b + 1)
}

fn md5_of(segments: &[Bytes]) -> [u8; 16] {
    let mut h = Md5::new();
    for s in segments {
        h.update(s);
    }
    h.finalize().into()
}

/// Composite multipart checksum: hash of the concatenated binary part checksums, suffixed `-N`.
fn composite_checksum(alg: &ChecksumAlgorithm, parts: &[&Checksum]) -> Checksum {
    let idx = match alg.as_str().to_ascii_uppercase().as_str() {
        ChecksumAlgorithm::CRC32 => 0,
        ChecksumAlgorithm::CRC32C => 1,
        ChecksumAlgorithm::SHA1 => 2,
        ChecksumAlgorithm::SHA256 => 3,
        _ => return Checksum::default(),
    };
    let mut hasher = hasher_for(&Checksum::default(), Some(alg)).unwrap_or_default();
    for p in parts {
        let mut p = (*p).clone();
        let Some(v) = checksum_slot(&mut p, idx).clone() else { return Checksum::default() };
        let Ok(raw) = base64_simd::STANDARD.decode_to_vec(v) else { return Checksum::default() };
        hasher.update(&raw);
    }
    let mut out = hasher.finalize();
    let slot = checksum_slot(&mut out, idx);
    if let Some(v) = slot.as_mut() {
        v.push_str(&format!("-{}", parts.len()));
    }
    out
}

/// `aws-chunked` describes the request framing, not the object; S3 never stores it.
fn strip_aws_chunked(ce: Option<String>) -> Option<String> {
    let ce = ce?;
    let kept: Vec<&str> = ce.split(',').map(str::trim).filter(|t| !t.is_empty() && !t.eq_ignore_ascii_case("aws-chunked")).collect();
    (!kept.is_empty()).then(|| kept.join(","))
}

fn meta_from(
    content_type: Option<impl ToString>,
    content_encoding: Option<String>,
    content_disposition: Option<String>,
    content_language: Option<String>,
    cache_control: Option<String>,
    expires: Option<String>,
    user_metadata: Option<std::collections::HashMap<String, String>>,
) -> ObjectMeta {
    ObjectMeta {
        content_type: content_type.map(|c| c.to_string()),
        content_encoding: strip_aws_chunked(content_encoding),
        content_disposition,
        content_language,
        cache_control,
        expires,
        user_metadata,
    }
}

impl FakeS3 {
    fn bucket(&self, name: &str) -> S3Result<Arc<StoreBucket>> {
        self.store.bucket(name).ok_or_else(|| s3_error!(NoSuchBucket))
    }

    /// Resolves the version a read addresses. Returns the version and whether the bucket reports
    /// version ids.
    fn version(&self, bucket: &str, key: &str, version_id: Option<&str>) -> S3Result<(Arc<Version>, bool)> {
        let b = self.bucket(bucket)?;
        let versioned = *b.versioning.read() != Versioning::Unversioned;
        let objects = b.objects.read();
        let entry = objects.get(key);
        let v = match version_id {
            None => entry.and_then(Entry::latest).cloned().ok_or_else(|| s3_error!(NoSuchKey))?,
            Some(id) => entry.and_then(|e| e.find(id)).cloned().ok_or_else(|| s3_error!(NoSuchVersion))?,
        };
        if v.obj.is_none() {
            let mut e = if version_id.is_some() { s3_error!(MethodNotAllowed) } else { s3_error!(NoSuchKey) };
            let mut h = http::HeaderMap::new();
            h.insert("x-amz-delete-marker", http::HeaderValue::from_static("true"));
            if versioned {
                if let Ok(id) = http::HeaderValue::from_str(&v.id) {
                    h.insert("x-amz-version-id", id);
                }
            }
            e.set_headers(h);
            return Err(e);
        }
        Ok((v, versioned))
    }

    fn source_object(&self, src: &CopySource) -> S3Result<(Arc<Version>, bool)> {
        let CopySource::Bucket { bucket, key, version_id } = src else {
            return Err(s3_error!(NotImplemented, "access point copy sources are not supported"));
        };
        self.version(bucket, key, version_id.as_deref())
    }
}

/// Byte range of part `n` (1-based). Objects not assembled from parts have a single part.
fn part_range(o: &StoreObject, n: i32) -> S3Result<std::ops::Range<u64>> {
    let sizes: &[u64] = match &o.part_sizes {
        Some(s) => s,
        None => std::slice::from_ref(&o.data.size),
    };
    if n < 1 || n as usize > sizes.len() {
        let mut e = S3Error::with_message(S3ErrorCode::Custom("InvalidPartNumber".into()), "The requested partnumber is not satisfiable");
        e.set_status_code(http::StatusCode::RANGE_NOT_SATISFIABLE);
        return Err(e);
    }
    let start: u64 = sizes[..n as usize - 1].iter().sum();
    Ok(start..start + sizes[n as usize - 1])
}

fn obj(v: &Version) -> &StoreObject {
    v.obj.as_ref().expect("resolved versions are never delete markers")
}

/// Checks a write's If-Match / If-None-Match against the key's current object.
fn check_write_conditions(entry: Option<&Entry>, if_match: Option<&ETagCondition>, if_none_match: Option<&ETagCondition>) -> S3Result<()> {
    match entry.and_then(Entry::current) {
        Some(existing) => {
            if if_none_match.is_some_and(ETagCondition::is_any) {
                return Err(s3_error!(PreconditionFailed));
            }
            if let Some(c) = if_match {
                check_conditions(existing, Some(c), None, None, None)?;
            }
        }
        None if if_match.is_some() => return Err(s3_error!(NoSuchKey)),
        None => {}
    }
    Ok(())
}

struct Listing {
    contents: Vec<Object>,
    prefixes: Vec<CommonPrefix>,
    truncated: bool,
    next: Option<String>,
}

/// Walks keys in order from `after` (exclusive) under `prefix`, folding keys that contain
/// `delimiter` after the prefix into common prefixes. Each emitted key or prefix is passed to
/// `emit`, which returns how many result slots it consumed (0 means "skip").
fn walk<F>(objects: &BTreeMap<String, Entry>, prefix: &str, delimiter: Option<&str>, after: Option<&str>, max_keys: usize, mut emit: F) -> (Vec<CommonPrefix>, bool, Option<String>)
where
    F: FnMut(&str, &Entry) -> usize,
{
    let mut prefixes = Vec::new();
    let mut used = 0usize;
    let mut next = None;
    let mut cursor: Bound<String> = match after {
        Some(a) if a >= prefix => Bound::Excluded(a.to_owned()),
        _ => Bound::Included(prefix.to_owned()),
    };
    let delimiter = delimiter.filter(|d| !d.is_empty());
    'outer: loop {
        for (key, entry) in objects.range((cursor.clone(), Bound::Unbounded)) {
            if !key.starts_with(prefix) {
                break 'outer;
            }
            if let Some(d) = delimiter {
                if let Some(pos) = key[prefix.len()..].find(d) {
                    if entry.latest().is_none() {
                        continue;
                    }
                    if used >= max_keys {
                        return (prefixes, true, next);
                    }
                    let cp = &key[..prefix.len() + pos + d.len()];
                    prefixes.push(CommonPrefix { prefix: Some(cp.to_owned()) });
                    used += 1;
                    next = Some(cp.to_owned());
                    let mut skip = cp.to_owned();
                    skip.push(char::MAX);
                    cursor = Bound::Excluded(skip);
                    continue 'outer;
                }
            }
            if used >= max_keys && emit_would_produce(entry) {
                return (prefixes, true, next);
            }
            let n = emit(key, entry);
            if n > 0 {
                used += n;
                next = Some(key.clone());
            }
        }
        break;
    }
    (prefixes, false, None)
}

fn emit_would_produce(entry: &Entry) -> bool {
    entry.latest().is_some()
}

fn list(objects: &BTreeMap<String, Entry>, prefix: &str, delimiter: Option<&str>, after: Option<&str>, max_keys: usize) -> Listing {
    let mut contents = Vec::new();
    // Keys whose latest version is a delete marker are invisible to plain listings.
    let visible: BTreeMap<String, Entry>;
    let objects = if objects.values().any(|e| e.current().is_none()) {
        visible = objects
            .iter()
            .filter(|(_, e)| e.current().is_some())
            .map(|(k, e)| (k.clone(), Entry { versions: e.versions.clone() }))
            .collect();
        &visible
    } else {
        objects
    };
    let (prefixes, truncated, next) = walk(objects, prefix, delimiter, after, max_keys, |key, entry| {
        let o = entry.current().expect("filtered above");
        contents.push(Object {
            key: Some(key.to_owned()),
            e_tag: Some(ETag::Strong(o.e_tag.clone())),
            last_modified: Some(ts(o.last_modified)),
            size: Some(o.data.size as i64),
            storage_class: Some(ObjectStorageClass::from_static(ObjectStorageClass::STANDARD)),
            ..Default::default()
        });
        1
    });
    Listing { contents, prefixes, truncated, next }
}

/// `encoding-type=url` escaping as S3 does it: form-style (space becomes `+`), `/` kept.
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn wants_url(e: &Option<EncodingType>) -> bool {
    e.as_ref().is_some_and(|e| e.as_str().eq_ignore_ascii_case("url"))
}

fn enc(o: &mut Option<String>) {
    if let Some(s) = o {
        *s = url_encode(s);
    }
}

fn enc_prefixes(p: &mut Option<Vec<CommonPrefix>>) {
    for cp in p.iter_mut().flatten() {
        enc(&mut cp.prefix);
    }
}

fn multipart_etag(md5s: &[[u8; 16]]) -> String {
    let mut h = Md5::new();
    for m in md5s {
        h.update(m);
    }
    format!("{}-{}", hex::encode(h.finalize()), md5s.len())
}

fn owner() -> Owner {
    Owner { display_name: Some("fakes3".into()), id: Some("fakes3".into()) }
}

#[async_trait::async_trait]
impl S3 for FakeS3 {
    async fn create_bucket(&self, req: S3Request<CreateBucketInput>) -> S3Result<S3Response<CreateBucketOutput>> {
        let name = req.input.bucket;
        validate_bucket_name(&name)?;
        let mut buckets = self.store.buckets.write();
        if buckets.contains_key(&name) {
            return Err(s3_error!(BucketAlreadyOwnedByYou));
        }
        buckets.insert(name.clone(), Arc::new(StoreBucket::new()));
        Ok(S3Response::new(CreateBucketOutput { location: Some(format!("/{name}")), ..Default::default() }))
    }

    async fn head_bucket(&self, req: S3Request<HeadBucketInput>) -> S3Result<S3Response<HeadBucketOutput>> {
        self.bucket(&req.input.bucket)?;
        Ok(S3Response::new(HeadBucketOutput { bucket_region: Some("us-east-1".into()), ..Default::default() }))
    }

    async fn delete_bucket(&self, req: S3Request<DeleteBucketInput>) -> S3Result<S3Response<DeleteBucketOutput>> {
        let mut buckets = self.store.buckets.write();
        let b = buckets.get(&req.input.bucket).ok_or_else(|| s3_error!(NoSuchBucket))?;
        if !b.objects.read().is_empty() {
            return Err(s3_error!(BucketNotEmpty));
        }
        buckets.remove(&req.input.bucket);
        Ok(S3Response::new(DeleteBucketOutput {}))
    }

    async fn get_bucket_location(&self, req: S3Request<GetBucketLocationInput>) -> S3Result<S3Response<GetBucketLocationOutput>> {
        self.bucket(&req.input.bucket)?;
        Ok(S3Response::new(GetBucketLocationOutput::default()))
    }

    async fn list_buckets(&self, _req: S3Request<ListBucketsInput>) -> S3Result<S3Response<ListBucketsOutput>> {
        let buckets = self
            .store
            .buckets
            .read()
            .iter()
            .map(|(name, b)| Bucket {
                name: Some(name.clone()),
                creation_date: Some(ts(b.created)),
                bucket_region: Some("us-east-1".into()),
                ..Default::default()
            })
            .collect();
        Ok(S3Response::new(ListBucketsOutput { buckets: Some(buckets), owner: Some(owner()), ..Default::default() }))
    }

    async fn put_bucket_versioning(&self, req: S3Request<PutBucketVersioningInput>) -> S3Result<S3Response<PutBucketVersioningOutput>> {
        let b = self.bucket(&req.input.bucket)?;
        let status = req.input.versioning_configuration.status.ok_or_else(|| s3_error!(MalformedXML))?;
        let mode = match status.as_str() {
            BucketVersioningStatus::ENABLED => Versioning::Enabled,
            BucketVersioningStatus::SUSPENDED => Versioning::Suspended,
            _ => return Err(s3_error!(MalformedXML)),
        };
        *b.versioning.write() = mode;
        Ok(S3Response::new(PutBucketVersioningOutput {}))
    }

    async fn get_bucket_versioning(&self, req: S3Request<GetBucketVersioningInput>) -> S3Result<S3Response<GetBucketVersioningOutput>> {
        let b = self.bucket(&req.input.bucket)?;
        let status = match *b.versioning.read() {
            Versioning::Unversioned => None,
            Versioning::Enabled => Some(BucketVersioningStatus::from_static(BucketVersioningStatus::ENABLED)),
            Versioning::Suspended => Some(BucketVersioningStatus::from_static(BucketVersioningStatus::SUSPENDED)),
        };
        Ok(S3Response::new(GetBucketVersioningOutput { status, ..Default::default() }))
    }

    async fn put_bucket_tagging(&self, req: S3Request<PutBucketTaggingInput>) -> S3Result<S3Response<PutBucketTaggingOutput>> {
        let b = self.bucket(&req.input.bucket)?;
        let tags = tags_from_set(req.input.tagging.tag_set)?;
        *b.tags.write() = Some(tags);
        Ok(S3Response::new(PutBucketTaggingOutput {}))
    }

    async fn get_bucket_tagging(&self, req: S3Request<GetBucketTaggingInput>) -> S3Result<S3Response<GetBucketTaggingOutput>> {
        let b = self.bucket(&req.input.bucket)?;
        let tags = b.tags.read().clone().ok_or_else(|| s3_error!(NoSuchTagSet))?;
        Ok(S3Response::new(GetBucketTaggingOutput { tag_set: tag_set(&tags) }))
    }

    async fn delete_bucket_tagging(&self, req: S3Request<DeleteBucketTaggingInput>) -> S3Result<S3Response<DeleteBucketTaggingOutput>> {
        let b = self.bucket(&req.input.bucket)?;
        *b.tags.write() = None;
        Ok(S3Response::new(DeleteBucketTaggingOutput {}))
    }

    async fn put_object(&self, req: S3Request<PutObjectInput>) -> S3Result<S3Response<PutObjectOutput>> {
        let input = req.input;
        let bucket = self.bucket(&input.bucket)?;
        let tags = parse_tagging_header(input.tagging.as_deref())?;
        let expected = checksum_from!(&input);
        let ing = ingest(
            self.inline_threshold,
            input.body,
            input.content_length,
            input.content_md5.as_deref(),
            expected,
            input.checksum_algorithm.as_ref(),
            req.trailing_headers,
        )
        .await?;
        let e_tag = hex::encode(ing.md5);
        let obj = StoreObject {
            data: ObjectData::from_segments(vec![ing.data]),
            meta: meta_from(
                input.content_type,
                input.content_encoding,
                input.content_disposition,
                input.content_language,
                input.cache_control,
                input.expires,
                input.metadata,
            ),
            e_tag: e_tag.clone(),
            last_modified: SystemTime::now(),
            checksum: ing.checksum.clone(),
            parts_count: None,
            part_sizes: None,
            tags,
        };
        let version_id = {
            let mut objects = bucket.objects.write();
            check_write_conditions(objects.get(&input.key), input.if_match.as_ref(), input.if_none_match.as_ref())?;
            bucket.put(&mut objects, input.key, obj)
        };
        Ok(S3Response::new(with_checksum!(PutObjectOutput { e_tag: Some(ETag::Strong(e_tag)), version_id: version_id }, ing.checksum)))
    }

    async fn get_object(&self, req: S3Request<GetObjectInput>) -> S3Result<S3Response<GetObjectOutput>> {
        let input = req.input;
        let (v, versioned) = self.version(&input.bucket, &input.key, input.version_id.as_deref())?;
        let o = obj(&v);
        check_conditions(o, input.if_match.as_ref(), input.if_none_match.as_ref(), input.if_modified_since.as_ref(), input.if_unmodified_since.as_ref())?;
        let size = o.data.size;
        let (range, content_range) = match (&input.range, input.part_number) {
            (Some(_), Some(_)) => return Err(s3_error!(InvalidRequest, "Cannot specify both Range header and partNumber query parameter")),
            (None, None) => (0..size, None),
            (Some(r), None) => {
                let r = r.check(size)?;
                let cr = format!("bytes {}-{}/{}", r.start, r.end.saturating_sub(1), size);
                (r, Some(cr))
            }
            (None, Some(n)) => {
                let r = part_range(o, n)?;
                let cr = format!("bytes {}-{}/{}", r.start, r.end.saturating_sub(1), size);
                (r, Some(cr))
            }
        };
        let segments = o.data.slice(range.clone());
        let body = match segments.len() {
            0 => StreamingBlob::from_bytes(Bytes::new()),
            1 => StreamingBlob::from_bytes(segments.into_iter().next().unwrap()),
            _ => StreamingBlob::wrap(futures::stream::iter(segments.into_iter().map(Ok::<_, std::io::Error>))),
        };
        // Like S3, only report checksums for whole-object reads when the client asks for them.
        let checksum = if content_range.is_none() && input.checksum_mode.is_some() { o.checksum.clone() } else { Checksum::default() };
        let m = &o.meta;
        let out = with_checksum!(
            GetObjectOutput {
                body: Some(body),
                content_length: Some((range.end - range.start) as i64),
                content_range: content_range,
                accept_ranges: Some("bytes".into()),
                e_tag: Some(ETag::Strong(o.e_tag.clone())),
                last_modified: Some(ts(o.last_modified)),
                content_type: input.response_content_type.or_else(|| m.content_type.clone()),
                content_encoding: input.response_content_encoding.or_else(|| m.content_encoding.clone()),
                content_disposition: input.response_content_disposition.or_else(|| m.content_disposition.clone()),
                content_language: input.response_content_language.or_else(|| m.content_language.clone()),
                cache_control: input.response_cache_control.or_else(|| m.cache_control.clone()),
                expires: m.expires.clone(),
                metadata: m.user_metadata.clone(),
                parts_count: o.parts_count,
                tag_count: tag_count(&o.tags),
                version_id: versioned.then(|| v.id.clone()),
            },
            checksum
        );
        Ok(S3Response::new(out))
    }

    async fn head_object(&self, req: S3Request<HeadObjectInput>) -> S3Result<S3Response<HeadObjectOutput>> {
        let input = req.input;
        let (v, versioned) = self.version(&input.bucket, &input.key, input.version_id.as_deref())?;
        let o = obj(&v);
        check_conditions(o, input.if_match.as_ref(), input.if_none_match.as_ref(), input.if_modified_since.as_ref(), input.if_unmodified_since.as_ref())?;
        let size = o.data.size;
        let range = match (&input.range, input.part_number) {
            (Some(_), Some(_)) => return Err(s3_error!(InvalidRequest, "Cannot specify both Range header and partNumber query parameter")),
            (Some(r), None) => Some(r.check(size)?),
            (None, Some(n)) => Some(part_range(o, n)?),
            (None, None) => None,
        };
        let (content_length, content_range) = match range {
            Some(r) => (r.end - r.start, Some(format!("bytes {}-{}/{}", r.start, r.end.saturating_sub(1), size))),
            None => (size, None),
        };
        let checksum = if input.checksum_mode.is_some() && content_range.is_none() { o.checksum.clone() } else { Checksum::default() };
        let partial = content_range.is_some();
        let m = &o.meta;
        let out = with_checksum!(
            HeadObjectOutput {
                content_length: Some(content_length as i64),
                content_range: content_range,
                accept_ranges: Some("bytes".into()),
                e_tag: Some(ETag::Strong(o.e_tag.clone())),
                last_modified: Some(ts(o.last_modified)),
                content_type: m.content_type.clone(),
                content_encoding: m.content_encoding.clone(),
                content_disposition: m.content_disposition.clone(),
                content_language: m.content_language.clone(),
                cache_control: m.cache_control.clone(),
                expires: m.expires.clone(),
                metadata: m.user_metadata.clone(),
                parts_count: o.parts_count,
                tag_count: tag_count(&o.tags),
                version_id: versioned.then(|| v.id.clone()),
            },
            checksum
        );
        let mut resp = S3Response::new(out);
        if partial {
            resp.status = Some(http::StatusCode::PARTIAL_CONTENT);
        }
        Ok(resp)
    }

    async fn delete_object(&self, req: S3Request<DeleteObjectInput>) -> S3Result<S3Response<DeleteObjectOutput>> {
        let input = req.input;
        let b = self.bucket(&input.bucket)?;
        let mut objects = b.objects.write();
        if let Some(c) = &input.if_match {
            match objects.get(&input.key).and_then(Entry::current) {
                Some(o) => check_conditions(o, Some(c), None, None, None)?,
                None => return Err(s3_error!(NoSuchKey)),
            }
        }
        let out = match input.version_id {
            Some(id) => {
                let marker = StoreBucket::delete_version(&mut objects, &input.key, &id);
                DeleteObjectOutput { version_id: Some(id), delete_marker: marker.filter(|m| *m), ..Default::default() }
            }
            None => match b.delete_current(&mut objects, &input.key) {
                Some(id) => DeleteObjectOutput { version_id: Some(id), delete_marker: Some(true), ..Default::default() },
                None => DeleteObjectOutput::default(),
            },
        };
        Ok(S3Response::new(out))
    }

    async fn delete_objects(&self, req: S3Request<DeleteObjectsInput>) -> S3Result<S3Response<DeleteObjectsOutput>> {
        let b = self.bucket(&req.input.bucket)?;
        let quiet = req.input.delete.quiet.unwrap_or(false);
        let mut deleted = Vec::new();
        {
            let mut objects = b.objects.write();
            for id in req.input.delete.objects {
                let d = match id.version_id {
                    Some(vid) => {
                        let marker = StoreBucket::delete_version(&mut objects, &id.key, &vid);
                        DeletedObject { key: Some(id.key), version_id: Some(vid.clone()), delete_marker: marker.filter(|m| *m), delete_marker_version_id: marker.filter(|m| *m).map(|_| vid) }
                    }
                    None => match b.delete_current(&mut objects, &id.key) {
                        Some(mid) => DeletedObject { key: Some(id.key), delete_marker: Some(true), delete_marker_version_id: Some(mid), ..Default::default() },
                        None => DeletedObject { key: Some(id.key), ..Default::default() },
                    },
                };
                if !quiet {
                    deleted.push(d);
                }
            }
        }
        Ok(S3Response::new(DeleteObjectsOutput { deleted: Some(deleted), ..Default::default() }))
    }

    async fn copy_object(&self, req: S3Request<CopyObjectInput>) -> S3Result<S3Response<CopyObjectOutput>> {
        let input = req.input;
        let (src_v, src_versioned) = self.source_object(&input.copy_source)?;
        let src = obj(&src_v);
        check_conditions(
            src,
            input.copy_source_if_match.as_ref(),
            input.copy_source_if_none_match.as_ref(),
            input.copy_source_if_modified_since.as_ref(),
            input.copy_source_if_unmodified_since.as_ref(),
        )
        .map_err(|e| if *e.code() == S3ErrorCode::NotModified { s3_error!(PreconditionFailed) } else { e })?;
        let dst_bucket = self.bucket(&input.bucket)?;
        let replace = input.metadata_directive.as_ref().is_some_and(|d| d.as_str() == MetadataDirective::REPLACE);
        let replace_tags = input.tagging_directive.as_ref().is_some_and(|d| d.as_str() == TaggingDirective::REPLACE);
        if let CopySource::Bucket { bucket, key, version_id: None } = &input.copy_source {
            if !replace && **bucket == *input.bucket && **key == *input.key {
                return Err(s3_error!(
                    InvalidRequest,
                    "This copy request is illegal because it is trying to copy an object to itself without changing the object's metadata, storage class, website redirect location or encryption attributes."
                ));
            }
        }
        let meta = if replace {
            meta_from(
                input.content_type,
                input.content_encoding,
                input.content_disposition,
                input.content_language,
                input.cache_control,
                input.expires,
                input.metadata,
            )
        } else {
            src.meta.clone()
        };
        let tags = if replace_tags { parse_tagging_header(input.tagging.as_deref())? } else { src.tags.clone() };
        let now = SystemTime::now();
        let new = StoreObject {
            data: src.data.clone(),
            meta,
            e_tag: src.e_tag.clone(),
            last_modified: now,
            checksum: src.checksum.clone(),
            parts_count: src.parts_count,
            part_sizes: src.part_sizes.clone(),
            tags,
        };
        let version_id = {
            let mut objects = dst_bucket.objects.write();
            dst_bucket.put(&mut objects, input.key, new)
        };
        Ok(S3Response::new(CopyObjectOutput {
            copy_object_result: Some(CopyObjectResult {
                e_tag: Some(ETag::Strong(src.e_tag.clone())),
                last_modified: Some(ts(now)),
                ..Default::default()
            }),
            version_id,
            copy_source_version_id: src_versioned.then(|| src_v.id.clone()),
            ..Default::default()
        }))
    }

    async fn put_object_tagging(&self, req: S3Request<PutObjectTaggingInput>) -> S3Result<S3Response<PutObjectTaggingOutput>> {
        let input = req.input;
        let tags = tags_from_set(input.tagging.tag_set)?;
        let b = self.bucket(&input.bucket)?;
        let versioned = *b.versioning.read() != Versioning::Unversioned;
        let mut objects = b.objects.write();
        let entry = objects.get_mut(&input.key).ok_or_else(|| s3_error!(NoSuchKey))?;
        let pos = match &input.version_id {
            Some(id) => entry.versions.iter().rposition(|v| &v.id == id).ok_or_else(|| s3_error!(NoSuchVersion))?,
            None => entry.versions.len() - 1,
        };
        let v = &entry.versions[pos];
        let Some(o) = &v.obj else {
            return Err(if input.version_id.is_some() { s3_error!(MethodNotAllowed) } else { s3_error!(NoSuchKey) });
        };
        let mut o = o.clone();
        o.tags = tags;
        let id = v.id.clone();
        entry.versions[pos] = Arc::new(Version { id: id.clone(), obj: Some(o), last_modified: v.last_modified });
        Ok(S3Response::new(PutObjectTaggingOutput { version_id: versioned.then_some(id) }))
    }

    async fn get_object_tagging(&self, req: S3Request<GetObjectTaggingInput>) -> S3Result<S3Response<GetObjectTaggingOutput>> {
        let input = req.input;
        let (v, versioned) = self.version(&input.bucket, &input.key, input.version_id.as_deref())?;
        Ok(S3Response::new(GetObjectTaggingOutput { tag_set: tag_set(&obj(&v).tags), version_id: versioned.then(|| v.id.clone()) }))
    }

    async fn delete_object_tagging(&self, req: S3Request<DeleteObjectTaggingInput>) -> S3Result<S3Response<DeleteObjectTaggingOutput>> {
        let req = req.map_input(|input| PutObjectTaggingInput {
            bucket: input.bucket,
            key: input.key,
            version_id: input.version_id,
            tagging: Tagging { tag_set: Vec::new() },
            checksum_algorithm: None,
            content_md5: None,
            expected_bucket_owner: None,
            request_payer: None,
        });
        let out = self.put_object_tagging(req).await?;
        Ok(S3Response::new(DeleteObjectTaggingOutput { version_id: out.output.version_id }))
    }

    async fn list_objects_v2(&self, req: S3Request<ListObjectsV2Input>) -> S3Result<S3Response<ListObjectsV2Output>> {
        let input = req.input;
        let b = self.bucket(&input.bucket)?;
        let prefix = input.prefix.clone().unwrap_or_default();
        let max_keys = input.max_keys.unwrap_or(1000).clamp(0, 1000) as usize;
        let token = match &input.continuation_token {
            Some(t) => Some(
                String::from_utf8(hex::decode(t).map_err(|_| s3_error!(InvalidArgument, "The continuation token provided is incorrect"))?)
                    .map_err(|_| s3_error!(InvalidArgument))?,
            ),
            None => None,
        };
        let after = token.as_deref().or(input.start_after.as_deref());
        let l = list(&b.objects.read(), &prefix, input.delimiter.as_deref(), after, max_keys);
        let key_count = (l.contents.len() + l.prefixes.len()) as i32;
        let mut out = ListObjectsV2Output {
            name: Some(input.bucket),
            prefix: Some(prefix),
            delimiter: input.delimiter,
            max_keys: Some(max_keys as i32),
            key_count: Some(key_count),
            is_truncated: Some(l.truncated),
            continuation_token: input.continuation_token,
            next_continuation_token: l.next.map(hex::encode),
            start_after: input.start_after,
            contents: Some(l.contents),
            common_prefixes: Some(l.prefixes),
            encoding_type: input.encoding_type,
            ..Default::default()
        };
        if wants_url(&out.encoding_type) {
            enc(&mut out.prefix);
            enc(&mut out.delimiter);
            enc(&mut out.start_after);
            out.contents.iter_mut().flatten().for_each(|o| enc(&mut o.key));
            enc_prefixes(&mut out.common_prefixes);
        }
        Ok(S3Response::new(out))
    }

    async fn list_objects(&self, req: S3Request<ListObjectsInput>) -> S3Result<S3Response<ListObjectsOutput>> {
        let input = req.input;
        let b = self.bucket(&input.bucket)?;
        let prefix = input.prefix.clone().unwrap_or_default();
        let max_keys = input.max_keys.unwrap_or(1000).clamp(0, 1000) as usize;
        let l = list(&b.objects.read(), &prefix, input.delimiter.as_deref(), input.marker.as_deref(), max_keys);
        let next_marker = if input.delimiter.is_some() { l.next } else { None };
        let mut out = ListObjectsOutput {
            name: Some(input.bucket),
            prefix: Some(prefix),
            delimiter: input.delimiter,
            marker: input.marker,
            max_keys: Some(max_keys as i32),
            is_truncated: Some(l.truncated),
            next_marker,
            contents: Some(l.contents),
            common_prefixes: Some(l.prefixes),
            encoding_type: input.encoding_type,
            ..Default::default()
        };
        if wants_url(&out.encoding_type) {
            enc(&mut out.prefix);
            enc(&mut out.delimiter);
            enc(&mut out.marker);
            enc(&mut out.next_marker);
            out.contents.iter_mut().flatten().for_each(|o| enc(&mut o.key));
            enc_prefixes(&mut out.common_prefixes);
        }
        Ok(S3Response::new(out))
    }

    async fn list_object_versions(&self, req: S3Request<ListObjectVersionsInput>) -> S3Result<S3Response<ListObjectVersionsOutput>> {
        let input = req.input;
        let b = self.bucket(&input.bucket)?;
        let prefix = input.prefix.clone().unwrap_or_default();
        let max_keys = input.max_keys.unwrap_or(1000).clamp(0, 1000) as usize;
        let objects = b.objects.read();
        let mut versions = Vec::new();
        let mut markers = Vec::new();
        let mut emitted = 0usize;
        let mut next_version: Option<(String, String)> = None;
        let mut truncated_mid_key = false;

        // A version-id marker resumes inside its key; otherwise the key marker is exclusive.
        let (after, resume) = match (&input.key_marker, &input.version_id_marker) {
            (Some(k), Some(v)) if !v.is_empty() => {
                let before = objects.range::<String, _>(..k.clone()).next_back().map(|(pk, _)| pk.clone());
                (before, Some((k.clone(), v.clone())))
            }
            (k, _) => (k.clone(), None),
        };
        let (prefixes, mut truncated, mut next_key) = walk(&objects, &prefix, input.delimiter.as_deref(), after.as_deref(), max_keys, |key, entry| {
            if truncated_mid_key {
                return 0;
            }
            let mut n = 0;
            let mut skipping = resume.as_ref().is_some_and(|(k, _)| k == key);
            for (i, v) in entry.versions.iter().rev().enumerate() {
                if skipping {
                    if resume.as_ref().is_some_and(|(_, vid)| *vid == v.id) {
                        skipping = false;
                    }
                    continue;
                }
                if emitted >= max_keys {
                    truncated_mid_key = true;
                    break;
                }
                let is_latest = Some(i == 0);
                match &v.obj {
                    Some(o) => versions.push(ObjectVersion {
                        key: Some(key.to_owned()),
                        version_id: Some(v.id.clone()),
                        is_latest,
                        e_tag: Some(ETag::Strong(o.e_tag.clone())),
                        last_modified: Some(ts(o.last_modified)),
                        size: Some(o.data.size as i64),
                        storage_class: Some(ObjectVersionStorageClass::from_static(ObjectVersionStorageClass::STANDARD)),
                        owner: Some(owner()),
                        ..Default::default()
                    }),
                    None => markers.push(DeleteMarkerEntry {
                        key: Some(key.to_owned()),
                        version_id: Some(v.id.clone()),
                        is_latest,
                        last_modified: Some(ts(v.last_modified)),
                        owner: Some(owner()),
                    }),
                }
                emitted += 1;
                n += 1;
                next_version = Some((key.to_owned(), v.id.clone()));
            }
            n
        });
        if truncated_mid_key {
            truncated = true;
            next_key = next_version.as_ref().map(|(k, _)| k.clone());
        }
        let next_version_id = if truncated { next_version.filter(|(k, _)| Some(k) == next_key.as_ref()).map(|(_, v)| v) } else { None };
        let mut out = ListObjectVersionsOutput {
            name: Some(input.bucket),
            prefix: Some(prefix),
            delimiter: input.delimiter,
            key_marker: input.key_marker,
            version_id_marker: input.version_id_marker,
            max_keys: Some(max_keys as i32),
            is_truncated: Some(truncated),
            next_key_marker: if truncated { next_key } else { None },
            next_version_id_marker: next_version_id,
            versions: Some(versions),
            delete_markers: Some(markers),
            common_prefixes: Some(prefixes),
            encoding_type: input.encoding_type,
            ..Default::default()
        };
        if wants_url(&out.encoding_type) {
            enc(&mut out.prefix);
            enc(&mut out.delimiter);
            enc(&mut out.key_marker);
            enc(&mut out.next_key_marker);
            out.versions.iter_mut().flatten().for_each(|o| enc(&mut o.key));
            out.delete_markers.iter_mut().flatten().for_each(|o| enc(&mut o.key));
            enc_prefixes(&mut out.common_prefixes);
        }
        Ok(S3Response::new(out))
    }

    async fn create_multipart_upload(&self, req: S3Request<CreateMultipartUploadInput>) -> S3Result<S3Response<CreateMultipartUploadOutput>> {
        let input = req.input;
        let b = self.bucket(&input.bucket)?;
        let tags = parse_tagging_header(input.tagging.as_deref())?;
        let upload_id = uuid::Uuid::new_v4().simple().to_string();
        b.uploads.write().insert(
            (input.key.clone(), upload_id.clone()),
            Upload {
                meta: meta_from(
                    input.content_type,
                    input.content_encoding,
                    input.content_disposition,
                    input.content_language,
                    input.cache_control,
                    input.expires,
                    input.metadata,
                ),
                tags,
                initiated: SystemTime::now(),
                checksum_algorithm: input.checksum_algorithm.clone(),
                parts: BTreeMap::new(),
            },
        );
        Ok(S3Response::new(CreateMultipartUploadOutput {
            bucket: Some(input.bucket),
            key: Some(input.key),
            upload_id: Some(upload_id),
            checksum_algorithm: input.checksum_algorithm,
            checksum_type: input.checksum_type,
            ..Default::default()
        }))
    }

    async fn upload_part(&self, req: S3Request<UploadPartInput>) -> S3Result<S3Response<UploadPartOutput>> {
        let input = req.input;
        let b = self.bucket(&input.bucket)?;
        if !(1..=10000).contains(&input.part_number) {
            return Err(s3_error!(InvalidArgument, "Part number must be an integer between 1 and 10000, inclusive"));
        }
        let id = (input.key.clone(), input.upload_id.clone());
        let algorithm = {
            let uploads = b.uploads.read();
            let up = uploads.get(&id).ok_or_else(|| s3_error!(NoSuchUpload))?;
            input.checksum_algorithm.clone().or_else(|| up.checksum_algorithm.clone())
        };
        let expected = checksum_from!(&input);
        let ing = ingest(
            self.inline_threshold,
            input.body,
            input.content_length,
            input.content_md5.as_deref(),
            expected,
            algorithm.as_ref(),
            req.trailing_headers,
        )
        .await?;
        let mut uploads = b.uploads.write();
        let up = uploads.get_mut(&id).ok_or_else(|| s3_error!(NoSuchUpload))?;
        up.parts.insert(
            input.part_number,
            Part {
                data: ObjectData::from_segments(vec![ing.data]),
                md5: ing.md5,
                last_modified: SystemTime::now(),
                checksum: ing.checksum.clone(),
            },
        );
        Ok(S3Response::new(with_checksum!(UploadPartOutput { e_tag: Some(ETag::Strong(hex::encode(ing.md5))) }, ing.checksum)))
    }

    async fn upload_part_copy(&self, req: S3Request<UploadPartCopyInput>) -> S3Result<S3Response<UploadPartCopyOutput>> {
        let input = req.input;
        let b = self.bucket(&input.bucket)?;
        if !(1..=10000).contains(&input.part_number) {
            return Err(s3_error!(InvalidArgument, "Part number must be an integer between 1 and 10000, inclusive"));
        }
        let (src_v, src_versioned) = self.source_object(&input.copy_source)?;
        let src = obj(&src_v);
        check_conditions(
            src,
            input.copy_source_if_match.as_ref(),
            input.copy_source_if_none_match.as_ref(),
            input.copy_source_if_modified_since.as_ref(),
            input.copy_source_if_unmodified_since.as_ref(),
        )
        .map_err(|e| if *e.code() == S3ErrorCode::NotModified { s3_error!(PreconditionFailed) } else { e })?;
        let range = match &input.copy_source_range {
            Some(r) => parse_copy_range(r, src.data.size)?,
            None => 0..src.data.size,
        };
        // The part shares the source's buffers; only the MD5 has to be computed.
        let segments = src.data.slice(range);
        let md5 = md5_of(&segments);
        let now = SystemTime::now();
        let mut uploads = b.uploads.write();
        let up = uploads.get_mut(&(input.key.clone(), input.upload_id.clone())).ok_or_else(|| s3_error!(NoSuchUpload))?;
        up.parts.insert(input.part_number, Part { data: ObjectData::from_segments(segments), md5, last_modified: now, checksum: Checksum::default() });
        Ok(S3Response::new(UploadPartCopyOutput {
            copy_part_result: Some(CopyPartResult { e_tag: Some(ETag::Strong(hex::encode(md5))), last_modified: Some(ts(now)), ..Default::default() }),
            copy_source_version_id: src_versioned.then(|| src_v.id.clone()),
            ..Default::default()
        }))
    }

    async fn list_parts(&self, req: S3Request<ListPartsInput>) -> S3Result<S3Response<ListPartsOutput>> {
        let input = req.input;
        let b = self.bucket(&input.bucket)?;
        let uploads = b.uploads.read();
        let up = uploads.get(&(input.key.clone(), input.upload_id.clone())).ok_or_else(|| s3_error!(NoSuchUpload))?;
        let max = input.max_parts.unwrap_or(1000).clamp(0, 1000) as usize;
        let marker = input.part_number_marker.unwrap_or(0);
        let mut parts: Vec<Part_> = Vec::new();
        let mut truncated = false;
        for (n, p) in up.parts.range(marker + 1..) {
            if parts.len() == max {
                truncated = true;
                break;
            }
            parts.push(with_checksum!(
                Part_ {
                    part_number: Some(*n),
                    e_tag: Some(ETag::Strong(hex::encode(p.md5))),
                    size: Some(p.data.size as i64),
                    last_modified: Some(ts(p.last_modified)),
                },
                p.checksum.clone()
            ));
        }
        let next = if truncated { parts.last().and_then(|p| p.part_number) } else { None };
        Ok(S3Response::new(ListPartsOutput {
            bucket: Some(input.bucket),
            key: Some(input.key),
            upload_id: Some(input.upload_id),
            max_parts: Some(max as i32),
            is_truncated: Some(truncated),
            part_number_marker: input.part_number_marker,
            next_part_number_marker: next,
            parts: Some(parts),
            checksum_algorithm: up.checksum_algorithm.clone(),
            owner: Some(owner()),
            initiator: Some(Initiator { display_name: Some("fakes3".into()), id: Some("fakes3".into()) }),
            storage_class: Some(StorageClass::from_static(StorageClass::STANDARD)),
            ..Default::default()
        }))
    }

    async fn list_multipart_uploads(&self, req: S3Request<ListMultipartUploadsInput>) -> S3Result<S3Response<ListMultipartUploadsOutput>> {
        let input = req.input;
        let b = self.bucket(&input.bucket)?;
        let prefix = input.prefix.clone().unwrap_or_default();
        let max = input.max_uploads.unwrap_or(1000).clamp(0, 1000) as usize;
        let uploads = b.uploads.read();
        let start = match (&input.key_marker, &input.upload_id_marker) {
            (Some(k), Some(u)) => Bound::Excluded((k.clone(), u.clone())),
            (Some(k), None) => Bound::Excluded((k.clone(), String::from(char::MAX))),
            _ => Bound::Unbounded,
        };
        let mut uploads_out = Vec::new();
        let mut prefixes: Vec<CommonPrefix> = Vec::new();
        let mut truncated = false;
        let mut last: Option<(String, String)> = None;
        let delimiter = input.delimiter.as_deref().filter(|d| !d.is_empty());
        for ((key, id), up) in uploads.range((start, Bound::Unbounded)) {
            if !key.starts_with(&prefix) {
                continue;
            }
            if uploads_out.len() + prefixes.len() >= max {
                truncated = true;
                break;
            }
            if let Some(d) = delimiter {
                if let Some(pos) = key[prefix.len()..].find(d) {
                    let cp = key[..prefix.len() + pos + d.len()].to_owned();
                    if prefixes.last().and_then(|p| p.prefix.as_ref()) != Some(&cp) {
                        prefixes.push(CommonPrefix { prefix: Some(cp) });
                    }
                    last = Some((key.clone(), id.clone()));
                    continue;
                }
            }
            uploads_out.push(MultipartUpload {
                key: Some(key.clone()),
                upload_id: Some(id.clone()),
                initiated: Some(ts(up.initiated)),
                storage_class: Some(StorageClass::from_static(StorageClass::STANDARD)),
                owner: Some(owner()),
                initiator: Some(Initiator { display_name: Some("fakes3".into()), id: Some("fakes3".into()) }),
                checksum_algorithm: up.checksum_algorithm.clone(),
                ..Default::default()
            });
            last = Some((key.clone(), id.clone()));
        }
        let (next_key, next_id) = if truncated { last.map(|(k, i)| (Some(k), Some(i))).unwrap_or_default() } else { (None, None) };
        let mut out = ListMultipartUploadsOutput {
            bucket: Some(input.bucket),
            prefix: Some(prefix),
            delimiter: input.delimiter,
            key_marker: input.key_marker,
            upload_id_marker: input.upload_id_marker,
            max_uploads: Some(max as i32),
            is_truncated: Some(truncated),
            next_key_marker: next_key,
            next_upload_id_marker: next_id,
            uploads: Some(uploads_out),
            common_prefixes: Some(prefixes),
            encoding_type: input.encoding_type,
            ..Default::default()
        };
        if wants_url(&out.encoding_type) {
            enc(&mut out.prefix);
            enc(&mut out.delimiter);
            enc(&mut out.key_marker);
            enc(&mut out.next_key_marker);
            out.uploads.iter_mut().flatten().for_each(|u| enc(&mut u.key));
            enc_prefixes(&mut out.common_prefixes);
        }
        Ok(S3Response::new(out))
    }

    async fn complete_multipart_upload(&self, req: S3Request<CompleteMultipartUploadInput>) -> S3Result<S3Response<CompleteMultipartUploadOutput>> {
        let input = req.input;
        let b = self.bucket(&input.bucket)?;
        let requested = input.multipart_upload.and_then(|m| m.parts).unwrap_or_default();
        if requested.is_empty() {
            return Err(s3_error!(MalformedXML, "The XML you provided was not well-formed or did not validate against our published schema"));
        }
        let id = (input.key.clone(), input.upload_id.clone());
        let (obj, checksum) = {
            let uploads = b.uploads.read();
            let up = uploads.get(&id).ok_or_else(|| s3_error!(NoSuchUpload))?;
            let mut segments = Vec::new();
            let mut md5s = Vec::with_capacity(requested.len());
            let mut sizes = Vec::with_capacity(requested.len());
            let mut checksums = Vec::with_capacity(requested.len());
            let mut last = 0;
            for (i, cp) in requested.iter().enumerate() {
                let n = cp.part_number.ok_or_else(|| s3_error!(InvalidPart))?;
                if n <= last {
                    return Err(s3_error!(InvalidPartOrder));
                }
                last = n;
                let p = up.parts.get(&n).ok_or_else(|| s3_error!(InvalidPart))?;
                if let Some(e) = &cp.e_tag {
                    if e.value().trim_matches('"') != hex::encode(p.md5) {
                        return Err(s3_error!(InvalidPart));
                    }
                }
                let claimed = checksum_from!(cp);
                for i in 0..TRAILER_NAMES.len() {
                    let (mut c, mut s) = (claimed.clone(), p.checksum.clone());
                    if let (Some(want), Some(have)) = (checksum_slot(&mut c, i).as_ref(), checksum_slot(&mut s, i).as_ref()) {
                        if want != have {
                            return Err(s3_error!(InvalidPart, "The checksum for part {n} did not match"));
                        }
                    }
                }
                if i + 1 < requested.len() && p.data.size < 5 * 1024 * 1024 {
                    return Err(s3_error!(EntityTooSmall, "Your proposed upload is smaller than the minimum allowed object size."));
                }
                segments.extend(p.data.segments.iter().cloned());
                sizes.push(p.data.size);
                md5s.push(p.md5);
                checksums.push(&p.checksum);
            }
            let checksum = up.checksum_algorithm.as_ref().map(|a| composite_checksum(a, &checksums)).unwrap_or_default();
            let obj = StoreObject {
                data: ObjectData::from_segments(segments),
                meta: up.meta.clone(),
                e_tag: multipart_etag(&md5s),
                last_modified: SystemTime::now(),
                checksum: checksum.clone(),
                parts_count: Some(md5s.len() as i32),
                part_sizes: Some(sizes.into()),
                tags: up.tags.clone(),
            };
            (obj, checksum)
        };
        let e_tag = obj.e_tag.clone();
        let version_id = {
            let mut objects = b.objects.write();
            check_write_conditions(objects.get(&input.key), input.if_match.as_ref(), input.if_none_match.as_ref())?;
            b.put(&mut objects, input.key.clone(), obj)
        };
        b.uploads.write().remove(&id);
        Ok(S3Response::new(with_checksum!(
            CompleteMultipartUploadOutput {
                bucket: Some(input.bucket.clone()),
                key: Some(input.key.clone()),
                location: Some(format!("/{}/{}", input.bucket, input.key)),
                e_tag: Some(ETag::Strong(e_tag)),
                version_id: version_id,
            },
            checksum
        )))
    }

    async fn abort_multipart_upload(&self, req: S3Request<AbortMultipartUploadInput>) -> S3Result<S3Response<AbortMultipartUploadOutput>> {
        let b = self.bucket(&req.input.bucket)?;
        if b.uploads.write().remove(&(req.input.key, req.input.upload_id)).is_none() {
            return Err(s3_error!(NoSuchUpload));
        }
        Ok(S3Response::new(AbortMultipartUploadOutput::default()))
    }
}

use s3s::dto::Part as Part_;
