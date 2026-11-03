use std::collections::BTreeMap;
use std::collections::HashSet;
use std::ops::Bound;
use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use futures::StreamExt;
use s3s::checksum::ChecksumHasher;
use s3s::dto::*;
use s3s::{S3, S3Error, S3ErrorCode, S3Request, S3Response, S3Result, s3_error};

use crate::access::{self, Identity};
use crate::acl;
use crate::bucket_config::Ownership;
use crate::bucket_defaults::BucketDefaults;
use crate::clock::Clock;
use crate::digest;
use crate::memory::{LimitExceeded, MemoryTracker};
use crate::noauth::Trailers;
use crate::objext::{self, apply_ext, apply_sse, object_opts};
use crate::objops;
use crate::store::{
    BlobError, BlobWriter, Bucket as StoreBucket, Entry, Index, Object as StoreObject, ObjectData, ObjectMeta, Part, Store, Upload,
    Version, Versioning,
};
use crate::users::{UserId, Users};

pub struct FakeS3 {
    pub store: Arc<Store>,
    pub inline_threshold: usize,
    pub memory: Arc<MemoryTracker>,
    pub clock: Arc<Clock>,
    pub defaults: BucketDefaults,
    pub users: Arc<Users>,
    /// Render list responses directly instead of through s3s' DTO serializer; see [`crate::listxml`].
    pub fast_list: bool,
}

impl FakeS3 {
    /// Who sent the request. With a single account nothing a handler does depends on it (the one
    /// account owns everything, whoever asks), so it is not looked up.
    fn who<T>(&self, req: &S3Request<T>) -> Identity {
        if !self.users.multi() {
            return Identity::User {
                id: UserId::DEFAULT,
                presigned: false,
            };
        }
        access::identify(&self.users, req)
    }

    /// The account that will own an object `who` writes into `bucket`. `canned` is the request's
    /// canned ACL: under `BucketOwnerPreferred`, `bucket-owner-full-control` hands ownership to the
    /// bucket owner. Anonymous writers (a public bucket) leave the object to the bucket owner.
    fn object_owner(&self, bucket: &StoreBucket, who: Identity, canned: Option<&str>) -> UserId {
        if !self.users.multi() {
            return bucket.owner;
        }
        let writer = who.user().unwrap_or(bucket.owner);
        match *bucket.config.ownership.read() {
            Some(Ownership::Enforced) => bucket.owner,
            Some(Ownership::Preferred) if canned == Some("bucket-owner-full-control") => bucket.owner,
            _ => writer,
        }
    }

    /// The accounts an ACL on something `owner` owns in `bucket` refers to.
    fn parties(&self, bucket: &StoreBucket, owner: UserId) -> acl::Parties<'_> {
        acl::Parties {
            users: &self.users,
            owner,
            bucket_owner: bucket.owner,
        }
    }

    fn initiator(&self, id: UserId) -> Initiator {
        let o = self.users.owner(id);
        Initiator {
            display_name: o.display_name,
            id: o.id,
        }
    }

    fn sink<'a>(&'a self, headers: &http::HeaderMap) -> Sink<'a> {
        Sink {
            threshold: self.inline_threshold,
            memory: &self.memory,
            // A client waiting for `100 Continue` hasn't sent the body yet; answer without asking for it.
            drain_rejected: !headers
                .get(http::header::EXPECT)
                .is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"100-continue")),
        }
    }
}

/// Where an incoming body is stored and how a refused one is handled.
struct Sink<'a> {
    threshold: usize,
    memory: &'a Arc<MemoryTracker>,
    /// Read and discard the rest of a body that is refused for lack of memory, so the client sees
    /// the error response rather than a connection reset while it is still uploading.
    drain_rejected: bool,
}

fn blob_err(e: BlobError) -> S3Error {
    match e {
        BlobError::Io(e) => s3_error!(e, InternalError),
        BlobError::Limit(l) => limit_err(&l),
    }
}

/// HTTP 507 with a fakes3-specific code, like MinIO's `XMinioStorageFull`. The status is
/// deliberately not 503 SlowDown (SDKs retry that with backoff, which only delays the failure)
/// and not 400 EntityTooLarge (it reads as a per-object size cap). None of the AWS SDKs'
/// default retry policies retry a 507.
fn limit_err(l: &LimitExceeded) -> S3Error {
    let code = "XFakeS3MemoryLimitExceeded".parse::<S3ErrorCode>().expect("infallible");
    let mut e = S3Error::with_message(code, l.to_string());
    e.set_status_code(http::StatusCode::INSUFFICIENT_STORAGE);
    e
}

async fn reject(body: Option<&mut StreamingBlob>, drain: bool, l: &LimitExceeded) -> S3Error {
    if drain && let Some(body) = body {
        while body.next().await.is_some_and(|c| c.is_ok()) {}
    }
    limit_err(l)
}

pub(crate) fn ts(t: SystemTime) -> Timestamp {
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
        #[allow(clippy::needless_update)]
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

/// One table drives everything that is per checksum algorithm: names, trailer headers, which
/// `Checksum` field holds the value and which `ChecksumHasher` field computes it.
macro_rules! algorithms {
    ($($idx:literal $name:literal $field:ident $hasher:ident;)*) => {
        const ALGS: &[(&str, &str)] = &[$(($name, concat!("x-amz-checksum-", stringify!($hasher))),)*];

        fn slot(c: &Checksum, i: usize) -> &Option<String> {
            match i { $($idx => &c.$field,)* _ => unreachable!() }
        }

        fn slot_mut(c: &mut Checksum, i: usize) -> &mut Option<String> {
            match i { $($idx => &mut c.$field,)* _ => unreachable!() }
        }

        fn enable(h: &mut ChecksumHasher, i: usize) {
            match i { $($idx => h.$hasher = Some(Default::default()),)* _ => unreachable!() }
        }
    };
}

algorithms! {
    0 "CRC32" checksum_crc32 crc32;
    1 "CRC32C" checksum_crc32c crc32c;
    2 "SHA1" checksum_sha1 sha1;
    3 "SHA256" checksum_sha256 sha256;
    4 "CRC64NVME" checksum_crc64nvme crc64nvme;
    5 "SHA512" checksum_sha512 sha512;
    6 "MD5" checksum_md5 md5;
    7 "XXHASH64" checksum_xxhash64 xxhash64;
    8 "XXHASH3" checksum_xxhash3 xxhash3;
    9 "XXHASH128" checksum_xxhash128 xxhash128;
}

const CRC32: usize = 0;
const CRC32C: usize = 1;
const CRC64NVME: usize = 4;

/// Index into `ALGS` for an algorithm name; SDKs differ in case (`crc32c` vs `CRC32C`).
fn alg_index(alg: &ChecksumAlgorithm) -> S3Result<usize> {
    ALGS.iter()
        .position(|(n, _)| n.eq_ignore_ascii_case(alg.as_str()))
        .ok_or_else(|| s3_error!(InvalidArgument, "Unsupported checksum algorithm: {}", alg.as_str()))
}

/// Enables every algorithm the client sent a value for or asked for. With none, computes
/// CRC64NVME, which S3 has stored for every new object by default since December 2024.
fn hasher_for(expected: &Checksum, algorithm: Option<&ChecksumAlgorithm>) -> S3Result<ChecksumHasher> {
    let mut h = ChecksumHasher::default();
    let mut any = false;
    for i in 0..ALGS.len() {
        if slot(expected, i).is_some() {
            enable(&mut h, i);
            any = true;
        }
    }
    if let Some(alg) = algorithm {
        enable(&mut h, alg_index(alg)?);
        any = true;
    }
    if !any {
        enable(&mut h, CRC64NVME);
    }
    Ok(h)
}

/// Fails with BadDigest if any checksum the client supplied differs from the computed one.
fn verify_checksums(expected: &Checksum, actual: &Checksum) -> S3Result<()> {
    for (i, (_, header)) in ALGS.iter().enumerate() {
        if let Some(want) = slot(expected, i)
            && slot(actual, i).as_deref() != Some(want.as_str())
        {
            return Err(bad_body_digest(format!(
                "The {header} you specified did not match the calculated checksum."
            )));
        }
    }
    Ok(())
}

/// BadDigest for a body that does not match what the client declared. A body that is not the
/// declared length (a short `Content-Length`) leaves stray bytes on the connection that would
/// corrupt the next request on it (`rPUT /k`), so, like S3, the connection is closed after the error.
fn bad_body_digest(message: String) -> S3Error {
    let mut e = S3Error::with_message(S3ErrorCode::BadDigest, message);
    let mut h = http::HeaderMap::new();
    h.insert(http::header::CONNECTION, http::HeaderValue::from_static("close"));
    // Headers set on an error replace the ones s3s would add, the content type among them.
    h.insert(http::header::CONTENT_TYPE, http::HeaderValue::from_static("application/xml"));
    e.set_headers(h);
    e
}

fn checksum_type(t: &'static str) -> ChecksumType {
    ChecksumType::from_static(t)
}

struct Ingested {
    data: Bytes,
    md5: [u8; 16],
    checksum: Checksum,
}

/// Reads a request body into a blob, validating Content-MD5 and any flexible checksums
/// (header or aws-chunked trailer) the client supplied.
async fn ingest(
    sink: Sink<'_>,
    mut body: Option<StreamingBlob>,
    content_length: Option<i64>,
    content_md5: Option<&str>,
    mut expected: Checksum,
    algorithm: Option<&ChecksumAlgorithm>,
    trailers: Option<Trailers>,
) -> S3Result<Ingested> {
    // A Content-MD5 that is not 16 base64-decoded bytes (or is empty) is InvalidDigest; BadDigest is
    // for a well-formed digest that does not match.
    let want_md5 = content_md5
        .map(|cm| {
            base64_simd::STANDARD
                .decode_to_vec(cm)
                .ok()
                .filter(|d| d.len() == 16)
                .ok_or_else(|| s3_error!(InvalidDigest, "The Content-MD5 you specified was invalid."))
        })
        .transpose()?;
    let mut hasher = hasher_for(&expected, algorithm)?;
    // Large bodies hash MD5 on another thread, overlapping with the work below.
    let mut md5 = digest::Streaming::new(content_length.is_some_and(|n| n as usize >= sink.threshold));
    // Refused here, before any of the body is read, when the declared length doesn't fit.
    let mut w = match BlobWriter::new(sink.memory, sink.threshold, content_length.and_then(|n| u64::try_from(n).ok())) {
        Ok(w) => w,
        Err(BlobError::Limit(l)) => return Err(reject(body.as_mut(), sink.drain_rejected, &l).await),
        Err(e) => return Err(blob_err(e)),
    };
    if let Some(body) = body.as_mut() {
        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(|e| s3_error!(IncompleteBody, "{e}"))?;
            md5.update(&chunk).await;
            hasher.update(&chunk);
            match w.write(&chunk) {
                Ok(()) => {}
                Err(BlobError::Limit(l)) => {
                    // Crossed the limit mid-stream (unknown length): free what was buffered.
                    drop(w);
                    return Err(reject(Some(body), sink.drain_rejected, &l).await);
                }
                Err(e) => return Err(blob_err(e)),
            }
        }
    }
    let data = w.finish().map_err(blob_err)?;
    if let Some(n) = content_length
        && n as usize != data.len()
    {
        return Err(s3_error!(IncompleteBody));
    }
    let md5 = md5.finish().await;

    if let Some(want) = want_md5
        && want != md5
    {
        return Err(bad_body_digest(
            "The Content-MD5 you specified did not match what we received.".to_owned(),
        ));
    }

    if let Some(t) = trailers.and_then(|t| t.take()) {
        for (i, (_, header)) in ALGS.iter().enumerate() {
            if let Some(v) = t.get(*header) {
                *slot_mut(&mut expected, i) = Some(v.to_str().map_err(|_| s3_error!(InvalidArgument))?.to_owned());
            }
        }
    }

    let checksum = hasher.finalize();
    verify_checksums(&expected, &checksum)?;
    Ok(Ingested { data, md5, checksum })
}

/// MD5 and checksums of data that is already stored (UploadPartCopy sources).
fn digest_segments(segments: &[Bytes], algorithm: Option<&ChecksumAlgorithm>) -> S3Result<([u8; 16], Checksum)> {
    let mut hasher = hasher_for(&Checksum::default(), algorithm)?;
    for s in segments {
        hasher.update(s);
    }
    Ok((digest::md5(segments), hasher.finalize()))
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
    } else if let Some(t) = if_unmodified_since
        && lm > *t
    {
        return Err(s3_error!(PreconditionFailed));
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
        if lm.format(TimestampFormat::HttpDate, &mut buf).is_ok()
            && let Ok(v) = http::HeaderValue::from_bytes(&buf)
        {
            h.insert(http::header::LAST_MODIFIED, v);
        }
        e.set_headers(h);
        return Err(e);
    }
    Ok(())
}

type Tags = Vec<(String, String)>;

fn parse_tagging_header(h: Option<&str>) -> S3Result<Tags> {
    let Some(h) = h else { return Ok(Vec::new()) };
    let mut tags: Tags = form_urlencoded::parse(h.as_bytes())
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    validate_tags(&tags)?;
    // S3 keeps the tags of an `x-amz-tagging` header ordered by key, not as they were sent.
    tags.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(tags)
}

fn tags_from_set(set: TagSet) -> S3Result<Tags> {
    let tags: Tags = set
        .into_iter()
        .map(|t| (t.key.unwrap_or_default(), t.value.unwrap_or_default()))
        .collect();
    validate_tags(&tags)?;
    Ok(tags)
}

fn validate_tags(tags: &Tags) -> S3Result<()> {
    if tags.len() > 10 {
        return Err(s3_error!(InvalidTag, "Object tags cannot be greater than 10"));
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
    tags.iter()
        .map(|(k, v)| Tag {
            key: Some(k.clone()),
            value: Some(v.clone()),
        })
        .collect()
}

fn tag_count(tags: &Tags) -> Option<i32> {
    (!tags.is_empty()).then_some(tags.len() as i32)
}

/// `x-amz-copy-source-range` only accepts `bytes=first-last`, fully inside the source.
fn parse_copy_range(r: &str, size: u64) -> S3Result<std::ops::Range<u64>> {
    match Range::parse(r) {
        Ok(Range::Int { first, last: Some(last) }) if last < size => Ok(first..last + 1),
        Ok(Range::Int { last: Some(_), .. }) => Err(s3_error!(InvalidRange)),
        _ => Err(s3_error!(
            InvalidArgument,
            "The x-amz-copy-source-range value must be of the form bytes=first-last where first and last are the zero-based offsets of the first and last bytes to copy"
        )),
    }
}

/// The checksum type a multipart upload ends up with: explicit, or S3's default for the
/// algorithm (CRC64NVME is full-object only; the rest default to composite).
fn upload_checksum_type(alg: Option<&ChecksumAlgorithm>, explicit: Option<&ChecksumType>) -> S3Result<ChecksumType> {
    let idx = alg.map(alg_index).transpose()?;
    let full = match explicit.map(|t| t.as_str().to_ascii_uppercase()) {
        Some(t) if t == ChecksumType::FULL_OBJECT => true,
        Some(t) if t == ChecksumType::COMPOSITE => false,
        Some(_) => return Err(s3_error!(InvalidRequest, "Invalid checksum type")),
        None => idx.is_none_or(|i| i == CRC64NVME),
    };
    match (idx, full) {
        (Some(i), true) if ![CRC32, CRC32C, CRC64NVME].contains(&i) => Err(s3_error!(
            InvalidRequest,
            "The FULL_OBJECT checksum type cannot be used with the {} checksum algorithm.",
            ALGS[i].0
        )),
        (Some(CRC64NVME), false) => Err(s3_error!(
            InvalidRequest,
            "The COMPOSITE checksum type cannot be used with the CRC64NVME checksum algorithm."
        )),
        (_, true) => Ok(checksum_type(ChecksumType::FULL_OBJECT)),
        (_, false) => Ok(checksum_type(ChecksumType::COMPOSITE)),
    }
}

/// The whole-object checksum of a completed multipart upload. Parts are `(checksum, size)`.
///
/// COMPOSITE: digest of the concatenated binary part digests, suffixed `-N`.
/// FULL_OBJECT: the CRC of the whole object, derived from the part CRCs with `crc-fast`'s
/// combine, so no object data is re-read.
fn multipart_checksum(alg_idx: usize, ty: &ChecksumType, parts: &[(&Checksum, u64)]) -> Checksum {
    let mut out = Checksum::default();
    let raw: Option<Vec<Vec<u8>>> = parts
        .iter()
        .map(|(c, _)| slot(c, alg_idx).as_ref().and_then(|v| base64_simd::STANDARD.decode_to_vec(v).ok()))
        .collect();
    let Some(raw) = raw else { return out };

    if ty.as_str() == ChecksumType::FULL_OBJECT {
        let alg = match alg_idx {
            CRC32 => crc_fast::CrcAlgorithm::Crc32IsoHdlc,
            CRC32C => crc_fast::CrcAlgorithm::Crc32Iscsi,
            CRC64NVME => crc_fast::CrcAlgorithm::Crc64Nvme,
            _ => return out,
        };
        let mut crc: Option<u64> = None;
        for (bytes, (_, len)) in raw.iter().zip(parts) {
            let part = bytes.iter().fold(0u64, |acc, b| (acc << 8) | u64::from(*b));
            crc = Some(match crc {
                None => part,
                Some(c) => crc_fast::checksum_combine(alg, c, part, *len),
            });
        }
        let width = if alg_idx == CRC64NVME { 8 } else { 4 };
        let crc = crc.unwrap_or(0).to_be_bytes();
        *slot_mut(&mut out, alg_idx) = Some(base64_simd::STANDARD.encode_to_string(&crc[8 - width..]));
    } else {
        let mut h = ChecksumHasher::default();
        enable(&mut h, alg_idx);
        for bytes in &raw {
            h.update(bytes);
        }
        out = h.finalize();
        if let Some(v) = slot_mut(&mut out, alg_idx) {
            v.push_str(&format!("-{}", parts.len()));
        }
    }
    out
}

/// `aws-chunked` describes the request framing, not the object; S3 never stores it. The rest of the
/// client's value is returned as sent, spacing included (`deflate, gzip` stays `deflate, gzip`).
fn strip_aws_chunked(ce: Option<String>) -> Option<String> {
    let ce = ce?;
    let framing = |t: &str| t.trim().eq_ignore_ascii_case("aws-chunked");
    if !ce.split(',').any(framing) {
        return (!ce.trim().is_empty()).then_some(ce);
    }
    let kept: Vec<&str> = ce.split(',').filter(|t| !framing(t) && !t.trim().is_empty()).collect();
    let kept = kept.join(",");
    let kept = kept.trim();
    (!kept.is_empty()).then(|| kept.to_owned())
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
    pub(crate) fn bucket(&self, name: &str) -> S3Result<Arc<StoreBucket>> {
        let b = self.store.bucket(name).ok_or_else(|| s3_error!(NoSuchBucket))?;
        if b.config.has_lifecycle.load(std::sync::atomic::Ordering::Acquire) {
            crate::lifecycle::tick(&b, self.clock.now());
        }
        Ok(b)
    }

    /// The `x-amz-expiration` value for `obj`, if its bucket has a lifecycle rule that expires it.
    fn expiration_header(&self, bucket: &str, key: &str, obj: &StoreObject) -> Option<String> {
        if !self.store.access.any_lifecycle.load(std::sync::atomic::Ordering::Relaxed) {
            return None;
        }
        let lc = self.store.bucket(bucket)?.config.lifecycle.read().clone()?;
        lc.expiration_header(key, obj)
    }

    /// Resolves the version a read addresses. Returns the version and whether the bucket reports
    /// version ids.
    pub(crate) fn version(&self, bucket: &str, key: &str, version_id: Option<&str>) -> S3Result<(Arc<Version>, bool)> {
        if let Some(id) = version_id {
            objext::check_version_id(id)?;
        }
        let b = self.bucket(bucket)?;
        let versioned = *b.versioning.read() != Versioning::Unversioned;
        let objects = b.objects.read();
        let entry = objects.get(key);
        let v = match version_id {
            None => entry.and_then(Entry::latest).cloned().ok_or_else(|| s3_error!(NoSuchKey))?,
            Some(id) => entry.and_then(|e| e.find(id)).cloned().ok_or_else(|| s3_error!(NoSuchVersion))?,
        };
        if v.obj.is_none() {
            let mut e = if version_id.is_some() {
                s3_error!(MethodNotAllowed)
            } else {
                s3_error!(NoSuchKey)
            };
            let mut h = http::HeaderMap::new();
            h.insert("x-amz-delete-marker", http::HeaderValue::from_static("true"));
            if versioned && let Ok(id) = http::HeaderValue::from_str(&v.id) {
                h.insert("x-amz-version-id", id);
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

fn delete_error(key: String, version_id: Option<String>, e: S3Error) -> s3s::dto::Error {
    s3s::dto::Error {
        code: Some(e.code().as_str().to_owned()),
        key: Some(key),
        message: e.message().map(str::to_owned),
        version_id,
    }
}

/// Byte range of part `n` (1-based). Objects not assembled from parts have a single part.
fn part_range(o: &StoreObject, n: i32) -> S3Result<std::ops::Range<u64>> {
    let sizes: &[u64] = match &o.part_sizes {
        Some(s) => s,
        None => std::slice::from_ref(&o.data.size),
    };
    if n < 1 || n as usize > sizes.len() {
        let mut e = S3Error::with_message(
            S3ErrorCode::Custom("InvalidPartNumber".into()),
            "The requested partnumber is not satisfiable",
        );
        e.set_status_code(http::StatusCode::RANGE_NOT_SATISFIABLE);
        return Err(e);
    }
    let start: u64 = sizes[..n as usize - 1].iter().sum();
    Ok(start..start + sizes[n as usize - 1])
}

/// What a read reports as `Content-Type`: the stored value, or S3's default for objects written
/// without one.
fn content_type(m: &ObjectMeta) -> Option<ContentType> {
    m.content_type.clone().or_else(|| Some("binary/octet-stream".to_owned()))
}

/// The checksum a GET / HEAD reports, and its type. Like S3, only when the client asks for it
/// (`x-amz-checksum-mode`), and not for a byte range. A `?partNumber=` read reports the part's
/// own checksum, with the object's checksum type.
fn read_checksum(o: &StoreObject, mode: bool, ranged: bool, part: Option<i32>) -> (Checksum, Option<ChecksumType>) {
    if !mode || ranged {
        return (Checksum::default(), None);
    }
    let checksum = match (part, o.ext.as_ref().and_then(|e| e.part_checksums.as_ref())) {
        (Some(n), Some(parts)) if o.part_sizes.is_some() => {
            usize::try_from(n - 1).ok().and_then(|i| parts.get(i)).cloned().unwrap_or_default()
        }
        _ => o.checksum.clone(),
    };
    (checksum, o.checksum_type.clone())
}

pub(crate) fn obj(v: &Version) -> &StoreObject {
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

/// How a walk ended.
pub(crate) struct ListEnd {
    pub truncated: bool,
    /// The last key or common prefix returned, when the listing was cut short.
    pub next: Option<String>,
    /// Result slots used: keys and common prefixes.
    pub used: usize,
}

/// Where a [`walk`] starts and stops.
struct Scan<'a> {
    prefix: &'a str,
    delimiter: Option<&'a str>,
    /// Exclusive.
    after: Option<&'a str>,
    max_keys: usize,
}

/// Walks keys in order from `after` (exclusive) under `prefix`, folding keys that contain
/// `delimiter` after the prefix into common prefixes, which go to `on_prefix`. Keys for which
/// `visible` is false are skipped without being copied or counted (a prefix only appears once a
/// visible key under it is reached). Each visible key is passed to `emit`, which returns how many
/// result slots it consumed.
fn walk<'a, V, F, P>(objects: &'a Index, scan: Scan<'_>, visible: V, mut emit: F, mut on_prefix: P) -> ListEnd
where
    V: Fn(&Entry) -> bool,
    F: FnMut(&'a str, &'a Entry) -> usize,
    P: FnMut(&str),
{
    let Scan {
        prefix,
        delimiter,
        after,
        max_keys,
    } = scan;
    if max_keys == 0 {
        return ListEnd {
            truncated: false,
            next: None,
            used: 0,
        };
    }
    let mut used = 0usize;
    // Borrowed from the index: copied only when the listing is cut short.
    let mut next: Option<&str> = None;
    let first = match after {
        Some(a) if a >= prefix => Bound::Excluded(a),
        _ => Bound::Included(prefix),
    };
    // Where to resume once a common prefix is folded: after every key under it.
    let mut skip = String::new();
    let mut folded = false;
    let delimiter = delimiter.filter(|d| !d.is_empty());
    // A marker that is itself a common prefix (the NextMarker / continuation token of a page that
    // ended on one) resumes after every key under it, so the prefix is not listed twice.
    if let (Some(a), Some(d)) = (after, delimiter)
        && a.len() > prefix.len()
        && a.starts_with(prefix)
        && a[prefix.len()..].find(d).is_some_and(|p| p + d.len() == a.len() - prefix.len())
    {
        skip.push_str(a);
        skip.push(char::MAX);
        folded = true;
    }
    'outer: loop {
        let from = if folded { Bound::Excluded(skip.as_str()) } else { first };
        for (key, entry) in objects.range::<str, _>((from, Bound::Unbounded)) {
            if !key.starts_with(prefix) {
                break 'outer;
            }
            if !visible(entry) {
                continue;
            }
            if used >= max_keys {
                return ListEnd {
                    truncated: true,
                    next: next.map(str::to_owned),
                    used,
                };
            }
            if let Some(pos) = delimiter.and_then(|d| key[prefix.len()..].find(d).map(|p| p + d.len())) {
                let cp = &key[..prefix.len() + pos];
                on_prefix(cp);
                used += 1;
                next = Some(cp);
                skip.clear();
                skip.push_str(cp);
                skip.push(char::MAX);
                folded = true;
                continue 'outer;
            }
            let n = emit(key, entry);
            if n > 0 {
                used += n;
                next = Some(key);
            }
        }
        break;
    }
    ListEnd {
        truncated: false,
        next: None,
        used,
    }
}

/// [`walk`] over the keys a plain listing shows: those whose latest version is not a delete
/// marker.
pub(crate) fn walk_list<'a>(
    objects: &'a Index,
    prefix: &str,
    delimiter: Option<&str>,
    after: Option<&str>,
    max_keys: usize,
    mut emit: impl FnMut(&'a str, &'a Version, &'a StoreObject),
    on_prefix: impl FnMut(&str),
) -> ListEnd {
    // Keys whose latest version is a delete marker are invisible to plain listings.
    let visible = |e: &Entry| e.current().is_some();
    walk(
        objects,
        Scan {
            prefix,
            delimiter,
            after,
            max_keys,
        },
        visible,
        |key, entry| {
            let v = entry.latest().expect("walk only emits visible entries");
            emit(key, v, v.obj.as_ref().expect("walk only emits visible entries"));
            1
        },
        on_prefix,
    )
}

/// `owners` is given when the listing reports an `Owner` per key.
fn list(objects: &Index, prefix: &str, delimiter: Option<&str>, after: Option<&str>, max_keys: usize, owners: Option<&Users>) -> Listing {
    // Growing a 1000-entry page by doubling copies it about once more.
    let mut contents = Vec::with_capacity(max_keys.min(objects.len()));
    let mut prefixes = Vec::new();
    let end = walk_list(
        objects,
        prefix,
        delimiter,
        after,
        max_keys,
        |key, v, o| {
            contents.push(Object {
                key: Some(key.to_owned()),
                e_tag: Some(ETag::Strong(o.e_tag.clone())),
                last_modified: Some(ts(o.last_modified)),
                size: Some(o.data.size as i64),
                storage_class: Some(match objext::storage_class(o.ext.as_deref()) {
                    Some(c) => ObjectStorageClass::from(c.as_str().to_owned()),
                    None => ObjectStorageClass::from_static(ObjectStorageClass::STANDARD),
                }),
                owner: owners.map(|users| users.owner(v.owner)),
                ..Default::default()
            });
        },
        |p| {
            prefixes.push(CommonPrefix {
                prefix: Some(p.to_owned()),
            });
        },
    );
    Listing {
        contents,
        prefixes,
        truncated: end.truncated,
        next: end.next,
    }
}

/// How a ListObjectVersions walk ended.
pub(crate) struct VersionsEnd {
    pub truncated: bool,
    pub next_key: Option<String>,
    pub next_version_id: Option<String>,
}

/// The walk behind ListObjectVersions. Every version of a visited key goes to `on_version` with
/// whether it is the latest, newest version first; keys fold into common prefixes like in a plain
/// listing. A listing can end in the middle of a key's versions.
pub(crate) fn walk_versions<'a>(
    objects: &'a Index,
    input: &ListObjectVersionsInput,
    prefix: &str,
    max_keys: usize,
    mut on_version: impl FnMut(&'a str, &'a Version, bool),
    on_prefix: impl FnMut(&str),
) -> VersionsEnd {
    let mut emitted = 0usize;
    let mut next_version: Option<(&str, &str)> = None;
    let mut truncated_mid_key = false;

    // A version-id marker resumes inside its key; otherwise the key marker is exclusive.
    let (after, resume) = match (&input.key_marker, &input.version_id_marker) {
        (Some(k), Some(v)) if !v.is_empty() => {
            let before = objects.range::<String, _>(..k.clone()).next_back().map(|(pk, _)| pk.clone());
            (before, Some((k.clone(), v.clone())))
        }
        (k, _) => (k.clone(), None),
    };
    let end = walk(
        objects,
        Scan {
            prefix,
            delimiter: input.delimiter.as_deref(),
            after: after.as_deref(),
            max_keys,
        },
        |_| true,
        |key, entry| {
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
                on_version(key, v, i == 0);
                emitted += 1;
                n += 1;
                next_version = Some((key, v.id.as_str()));
            }
            n
        },
        on_prefix,
    );
    let (mut truncated, mut next_key) = (end.truncated, end.next);
    if truncated_mid_key {
        truncated = true;
        next_key = next_version.map(|(k, _)| k.to_owned());
    }
    let next_version_id = if truncated {
        next_version
            .filter(|(k, _)| Some(*k) == next_key.as_deref())
            .map(|(_, v)| v.to_owned())
    } else {
        None
    };
    VersionsEnd {
        truncated,
        next_key: if truncated { next_key } else { None },
        next_version_id,
    }
}

/// An empty `output` that carries `body`, the response [`crate::listxml`] rendered, for `lib.rs` to
/// put in place of the one s3s writes for it.
fn pre_rendered<T>(output: T, body: Bytes) -> S3Response<T> {
    let mut resp = S3Response::new(output);
    resp.extensions.insert(crate::listxml::PreRendered(body));
    resp
}

/// `encoding-type=url` escaping: percent-encoding, a space is `%20` (percent-only decoders such as
/// botocore's and form decoders such as `URLDecoder` both read it; `+` would only survive the
/// latter) and a literal `+` is `%2B`. `/` stays literal like S3 and MinIO leave it; some clients
/// (minio-dotnet) don't decode common prefixes.
pub(crate) fn url_encode(s: &str) -> String {
    form_urlencoded::byte_serialize(s.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
        .replace("%2F", "/")
}

pub(crate) fn wants_url(e: &Option<EncodingType>) -> bool {
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
    format!("{}-{}", hex::encode(digest::md5(md5s)), md5s.len())
}

#[async_trait::async_trait]
impl S3 for FakeS3 {
    async fn create_bucket(&self, req: S3Request<CreateBucketInput>) -> S3Result<S3Response<CreateBucketOutput>> {
        let owner = self.who(&req).user().unwrap_or(UserId::DEFAULT);
        let input = req.input;
        let name = input.bucket.clone();
        let bucket = self.defaults.new_bucket(self.clock.now(), owner);
        if let Some(o) = &input.object_ownership {
            let o = Ownership::parse(o.as_str()).ok_or_else(|| s3_error!(InvalidArgument, "Invalid x-amz-object-ownership"))?;
            *bucket.config.ownership.write() = Some(o);
        }
        if let Some(acl) = acl::resolve_for_new_bucket(&bucket.config, &acl::acl_args!(&input, bucket), self.parties(&bucket, owner))? {
            *bucket.config.acl.write() = Some(acl);
        }
        if input.object_lock_enabled_for_bucket == Some(true) {
            // Object Lock needs versioning, which it keeps on for good.
            *bucket.versioning.write() = Versioning::Enabled;
            bucket.cfg.enable_lock();
        }
        if !self.store.add_bucket(name.clone(), bucket) {
            return Err(s3_error!(BucketAlreadyExists));
        }
        Ok(S3Response::new(CreateBucketOutput {
            location: Some(format!("/{name}")),
            ..Default::default()
        }))
    }

    async fn head_bucket(&self, req: S3Request<HeadBucketInput>) -> S3Result<S3Response<HeadBucketOutput>> {
        self.bucket(&req.input.bucket)?;
        Ok(S3Response::new(HeadBucketOutput {
            bucket_region: Some("us-east-1".into()),
            ..Default::default()
        }))
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

    async fn list_buckets(&self, req: S3Request<ListBucketsInput>) -> S3Result<S3Response<ListBucketsOutput>> {
        // Only the caller's own buckets.
        let me = self.who(&req).user().unwrap_or(UserId::DEFAULT);
        let input = req.input;
        // Without max-buckets the page is the whole account, unless one of the other parameters
        // asks for pagination, which then has AWS's default page size.
        let paginated =
            input.max_buckets.is_some() || input.continuation_token.is_some() || input.bucket_region.is_some() || input.prefix.is_some();
        let limit = match input.max_buckets {
            Some(n) if !(1..=10_000).contains(&n) => {
                return Err(s3_error!(
                    InvalidArgument,
                    "Argument max-buckets must be an integer between 1 and 10000"
                ));
            }
            Some(n) => n as usize,
            None if paginated => 10_000,
            None => usize::MAX,
        };
        // The token is the name of the last bucket of the previous page, hex-encoded to look opaque.
        let after = match &input.continuation_token {
            Some(t) => Some(
                hex::decode(t)
                    .ok()
                    .and_then(|b| String::from_utf8(b).ok())
                    .ok_or_else(|| s3_error!(InvalidArgument, "The continuation token provided is incorrect"))?,
            ),
            None => None,
        };
        let prefix = input.prefix.as_deref().unwrap_or("");
        // Every bucket lives in us-east-1.
        let region_matches = input.bucket_region.as_deref().is_none_or(|r| r == "us-east-1");
        let all = self.store.buckets.read();
        let from = after.as_deref().map_or(Bound::Unbounded, Bound::Excluded);
        let mut matching = all
            .range::<str, _>((from, Bound::Unbounded))
            .filter(|(name, b)| b.owner == me && region_matches && name.starts_with(prefix));
        let buckets: Vec<Bucket> = matching
            .by_ref()
            .take(limit)
            .map(|(name, b)| Bucket {
                name: Some(name.clone()),
                creation_date: Some(ts(b.created)),
                bucket_region: Some("us-east-1".into()),
                ..Default::default()
            })
            .collect();
        let continuation_token = matching
            .next()
            .and_then(|_| buckets.last())
            .and_then(|b| b.name.as_deref().map(hex::encode));
        Ok(S3Response::new(ListBucketsOutput {
            buckets: Some(buckets),
            owner: Some(self.users.owner(me)),
            continuation_token,
            prefix: input.prefix.filter(|p| !p.is_empty()),
        }))
    }

    /// S3 retired BitTorrent (new use ended in 2021) and answers `?torrent` with 404 NoSuchKey; the
    /// bucket and the key are checked first, so a missing bucket is still NoSuchBucket.
    async fn get_object_torrent(&self, req: S3Request<GetObjectTorrentInput>) -> S3Result<S3Response<GetObjectTorrentOutput>> {
        self.version(&req.input.bucket, &req.input.key, None)?;
        Err(s3_error!(NoSuchKey, "The specified key does not exist."))
    }

    async fn put_bucket_versioning(&self, req: S3Request<PutBucketVersioningInput>) -> S3Result<S3Response<PutBucketVersioningOutput>> {
        let b = self.bucket(&req.input.bucket)?;
        let conf = req.input.versioning_configuration;
        let status = conf.status.ok_or_else(|| s3_error!(MalformedXML))?;
        let mode = match status.as_str() {
            BucketVersioningStatus::ENABLED => Versioning::Enabled,
            BucketVersioningStatus::SUSPENDED => Versioning::Suspended,
            _ => return Err(s3_error!(MalformedXML)),
        };
        let mfa_delete = match conf.mfa_delete.as_ref().map(|m| m.as_str()) {
            None => None,
            Some(MFADelete::ENABLED) => Some(true),
            Some(MFADelete::DISABLED) => Some(false),
            Some(_) => return Err(s3_error!(MalformedXML)),
        };
        // Turning MFA Delete on, or touching a bucket that has it on, takes a token; `Disabled` on a
        // bucket where it is off (what clients send to keep it that way) does not.
        if mfa_delete == Some(true) || b.cfg.mfa_delete() == Some(true) {
            match req.input.mfa.as_deref() {
                None => return Err(s3_error!(AccessDenied, "Mfa Authentication must be used for this request")),
                Some(m) => objops::check_mfa_header(m)?,
            }
        }
        if mode == Versioning::Suspended && b.cfg.lock_enabled() {
            return Err(s3_error!(
                InvalidBucketState,
                "An Object Lock configuration is present on this bucket, so the versioning state cannot be changed."
            ));
        }
        if let Some(on) = mfa_delete {
            b.cfg.set_mfa_delete(on);
        }
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
        let mfa_delete = b
            .cfg
            .mfa_delete()
            .map(|on| MFADeleteStatus::from_static(if on { MFADeleteStatus::ENABLED } else { MFADeleteStatus::DISABLED }));
        Ok(S3Response::new(GetBucketVersioningOutput { status, mfa_delete }))
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

    // Bucket configuration (ACL, ownership, public access block, policy, CORS, lifecycle, website,
    // notification); the handlers live in bucket_ops.rs.
    async fn get_bucket_acl(&self, req: S3Request<GetBucketAclInput>) -> S3Result<S3Response<GetBucketAclOutput>> {
        self.do_get_bucket_acl(req)
    }

    async fn put_bucket_acl(&self, req: S3Request<PutBucketAclInput>) -> S3Result<S3Response<PutBucketAclOutput>> {
        self.do_put_bucket_acl(req)
    }

    async fn get_object_acl(&self, req: S3Request<GetObjectAclInput>) -> S3Result<S3Response<GetObjectAclOutput>> {
        self.do_get_object_acl(req)
    }

    async fn put_object_acl(&self, req: S3Request<PutObjectAclInput>) -> S3Result<S3Response<PutObjectAclOutput>> {
        self.do_put_object_acl(req)
    }

    async fn get_bucket_ownership_controls(
        &self,
        req: S3Request<GetBucketOwnershipControlsInput>,
    ) -> S3Result<S3Response<GetBucketOwnershipControlsOutput>> {
        self.do_get_bucket_ownership_controls(req)
    }

    async fn put_bucket_ownership_controls(
        &self,
        req: S3Request<PutBucketOwnershipControlsInput>,
    ) -> S3Result<S3Response<PutBucketOwnershipControlsOutput>> {
        self.do_put_bucket_ownership_controls(req)
    }

    async fn delete_bucket_ownership_controls(
        &self,
        req: S3Request<DeleteBucketOwnershipControlsInput>,
    ) -> S3Result<S3Response<DeleteBucketOwnershipControlsOutput>> {
        self.do_delete_bucket_ownership_controls(req)
    }

    async fn get_public_access_block(&self, req: S3Request<GetPublicAccessBlockInput>) -> S3Result<S3Response<GetPublicAccessBlockOutput>> {
        self.do_get_public_access_block(req)
    }

    async fn put_public_access_block(&self, req: S3Request<PutPublicAccessBlockInput>) -> S3Result<S3Response<PutPublicAccessBlockOutput>> {
        self.do_put_public_access_block(req)
    }

    async fn delete_public_access_block(
        &self,
        req: S3Request<DeletePublicAccessBlockInput>,
    ) -> S3Result<S3Response<DeletePublicAccessBlockOutput>> {
        self.do_delete_public_access_block(req)
    }

    async fn get_bucket_policy(&self, req: S3Request<GetBucketPolicyInput>) -> S3Result<S3Response<GetBucketPolicyOutput>> {
        self.do_get_bucket_policy(req)
    }

    async fn put_bucket_policy(&self, req: S3Request<PutBucketPolicyInput>) -> S3Result<S3Response<PutBucketPolicyOutput>> {
        self.do_put_bucket_policy(req)
    }

    async fn delete_bucket_policy(&self, req: S3Request<DeleteBucketPolicyInput>) -> S3Result<S3Response<DeleteBucketPolicyOutput>> {
        self.do_delete_bucket_policy(req)
    }

    async fn get_bucket_policy_status(
        &self,
        req: S3Request<GetBucketPolicyStatusInput>,
    ) -> S3Result<S3Response<GetBucketPolicyStatusOutput>> {
        self.do_get_bucket_policy_status(req)
    }

    async fn get_bucket_cors(&self, req: S3Request<GetBucketCorsInput>) -> S3Result<S3Response<GetBucketCorsOutput>> {
        self.do_get_bucket_cors(req)
    }

    async fn put_bucket_cors(&self, req: S3Request<PutBucketCorsInput>) -> S3Result<S3Response<PutBucketCorsOutput>> {
        self.do_put_bucket_cors(req)
    }

    async fn delete_bucket_cors(&self, req: S3Request<DeleteBucketCorsInput>) -> S3Result<S3Response<DeleteBucketCorsOutput>> {
        self.do_delete_bucket_cors(req)
    }

    async fn get_bucket_lifecycle_configuration(
        &self,
        req: S3Request<GetBucketLifecycleConfigurationInput>,
    ) -> S3Result<S3Response<GetBucketLifecycleConfigurationOutput>> {
        self.do_get_bucket_lifecycle(req)
    }

    async fn put_bucket_lifecycle_configuration(
        &self,
        req: S3Request<PutBucketLifecycleConfigurationInput>,
    ) -> S3Result<S3Response<PutBucketLifecycleConfigurationOutput>> {
        self.do_put_bucket_lifecycle(req)
    }

    async fn delete_bucket_lifecycle(
        &self,
        req: S3Request<DeleteBucketLifecycleInput>,
    ) -> S3Result<S3Response<DeleteBucketLifecycleOutput>> {
        self.do_delete_bucket_lifecycle(req)
    }

    async fn get_bucket_website(&self, req: S3Request<GetBucketWebsiteInput>) -> S3Result<S3Response<GetBucketWebsiteOutput>> {
        self.do_get_bucket_website(req)
    }

    async fn put_bucket_website(&self, req: S3Request<PutBucketWebsiteInput>) -> S3Result<S3Response<PutBucketWebsiteOutput>> {
        self.do_put_bucket_website(req)
    }

    async fn delete_bucket_website(&self, req: S3Request<DeleteBucketWebsiteInput>) -> S3Result<S3Response<DeleteBucketWebsiteOutput>> {
        self.do_delete_bucket_website(req)
    }

    async fn get_bucket_notification_configuration(
        &self,
        req: S3Request<GetBucketNotificationConfigurationInput>,
    ) -> S3Result<S3Response<GetBucketNotificationConfigurationOutput>> {
        self.do_get_bucket_notification(req)
    }

    async fn put_bucket_notification_configuration(
        &self,
        req: S3Request<PutBucketNotificationConfigurationInput>,
    ) -> S3Result<S3Response<PutBucketNotificationConfigurationOutput>> {
        self.do_put_bucket_notification(req)
    }

    async fn put_object(&self, req: S3Request<PutObjectInput>) -> S3Result<S3Response<PutObjectOutput>> {
        let who = self.who(&req);
        let input = req.input;
        let bucket = self.bucket(&input.bucket)?;
        let tags = parse_tagging_header(input.tagging.as_deref())?;
        let owner = self.object_owner(&bucket, who, input.acl.as_ref().map(|a| a.as_str()));
        let acl = acl::resolve(&bucket.config, &acl::acl_args!(&input), false, self.parties(&bucket, owner))?.map(Arc::new);
        let expected = checksum_from!(&input);
        let ext = objext::resolve(object_opts!(input), &bucket, &self.clock)?;
        let ing = ingest(
            self.sink(&req.headers),
            input.body,
            input.content_length,
            input.content_md5.as_deref(),
            expected,
            input.checksum_algorithm.as_ref(),
            Trailers::of(req.trailing_headers, &req.extensions),
        )
        .await?;
        let e_tag = objext::etag_for(&ing.md5, ext.as_ref());
        let last_modified = self.clock.now();
        let ext = objext::finish(ext, &bucket, last_modified);
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
            last_modified,
            checksum: ing.checksum.clone(),
            checksum_type: Some(checksum_type(ChecksumType::FULL_OBJECT)),
            parts_count: None,
            part_sizes: None,
            tags,
            ext: ext.clone(),
            acl,
        };
        let expiration = self.expiration_header(&input.bucket, &input.key, &obj);
        let version_id = {
            let mut objects = bucket.objects.write();
            check_write_conditions(objects.get(&input.key), input.if_match.as_ref(), input.if_none_match.as_ref())?;
            bucket.put(&mut objects, input.key, obj, owner)
        };
        let mut out = with_checksum!(
            PutObjectOutput {
                e_tag: Some(ETag::Strong(e_tag)),
                expiration: expiration,
                version_id: version_id,
                checksum_type: Some(checksum_type(ChecksumType::FULL_OBJECT)),
            },
            ing.checksum
        );
        apply_sse!(out, objext::sse_out(ext.as_deref()));
        out.ssekms_encryption_context = objext::sse_context(ext.as_deref());
        Ok(S3Response::new(out))
    }

    async fn get_object(&self, req: S3Request<GetObjectInput>) -> S3Result<S3Response<GetObjectOutput>> {
        let input = req.input;
        let (v, versioned) = self.version(&input.bucket, &input.key, input.version_id.as_deref())?;
        let o = obj(&v);
        objext::check_read_key(
            o.ext.as_deref(),
            input.sse_customer_algorithm.as_deref(),
            input.sse_customer_key.as_deref(),
            input.sse_customer_key_md5.as_deref(),
        )?;
        objext::reject_write_headers(&req.headers, o.ext.as_deref())?;
        let archive = self.archive_state(&input.bucket, &input.key, o);
        if let Some(a) = &archive {
            a.require_readable(false)?;
        }
        check_conditions(
            o,
            input.if_match.as_ref(),
            input.if_none_match.as_ref(),
            input.if_modified_since.as_ref(),
            input.if_unmodified_since.as_ref(),
        )?;
        let size = o.data.size;
        let (range, content_range) = match (&input.range, input.part_number) {
            (Some(_), Some(_)) => {
                return Err(s3_error!(
                    InvalidRequest,
                    "Cannot specify both Range header and partNumber query parameter"
                ));
            }
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
        let (checksum, ctype) = read_checksum(o, input.checksum_mode.is_some(), input.range.is_some(), input.part_number);
        let m = &o.meta;
        let mut out = with_checksum!(
            GetObjectOutput {
                body: Some(body),
                content_length: Some((range.end - range.start) as i64),
                content_range: content_range,
                accept_ranges: Some("bytes".into()),
                e_tag: Some(ETag::Strong(o.e_tag.clone())),
                last_modified: Some(ts(o.last_modified)),
                content_type: input.response_content_type.or_else(|| content_type(m)),
                content_encoding: input.response_content_encoding.or_else(|| m.content_encoding.clone()),
                content_disposition: input.response_content_disposition.or_else(|| m.content_disposition.clone()),
                content_language: input.response_content_language.or_else(|| m.content_language.clone()),
                cache_control: input.response_cache_control.or_else(|| m.cache_control.clone()),
                expires: m.expires.clone(),
                metadata: m.user_metadata.clone(),
                parts_count: o.parts_count,
                tag_count: tag_count(&o.tags),
                version_id: versioned.then(|| v.id.clone()),
                checksum_type: ctype,
                expiration: self.expiration_header(&input.bucket, &input.key, o),
            },
            checksum
        );
        apply_ext!(out, o.ext.as_deref());
        out.restore = archive.and_then(|a| a.restore_header());
        Ok(S3Response::new(out))
    }

    async fn head_object(&self, req: S3Request<HeadObjectInput>) -> S3Result<S3Response<HeadObjectOutput>> {
        let input = req.input;
        let (v, versioned) = self.version(&input.bucket, &input.key, input.version_id.as_deref())?;
        let o = obj(&v);
        objext::check_read_key(
            o.ext.as_deref(),
            input.sse_customer_algorithm.as_deref(),
            input.sse_customer_key.as_deref(),
            input.sse_customer_key_md5.as_deref(),
        )?;
        objext::reject_write_headers(&req.headers, o.ext.as_deref())?;
        check_conditions(
            o,
            input.if_match.as_ref(),
            input.if_none_match.as_ref(),
            input.if_modified_since.as_ref(),
            input.if_unmodified_since.as_ref(),
        )?;
        let size = o.data.size;
        let range = match (&input.range, input.part_number) {
            (Some(_), Some(_)) => {
                return Err(s3_error!(
                    InvalidRequest,
                    "Cannot specify both Range header and partNumber query parameter"
                ));
            }
            (Some(r), None) => Some(r.check(size)?),
            (None, Some(n)) => Some(part_range(o, n)?),
            (None, None) => None,
        };
        let (content_length, content_range) = match range {
            Some(r) => (
                r.end - r.start,
                Some(format!("bytes {}-{}/{}", r.start, r.end.saturating_sub(1), size)),
            ),
            None => (size, None),
        };
        let (checksum, ctype) = read_checksum(o, input.checksum_mode.is_some(), input.range.is_some(), input.part_number);
        let partial = content_range.is_some();
        let m = &o.meta;
        let mut out = with_checksum!(
            HeadObjectOutput {
                content_length: Some(content_length as i64),
                content_range: content_range,
                accept_ranges: Some("bytes".into()),
                e_tag: Some(ETag::Strong(o.e_tag.clone())),
                last_modified: Some(ts(o.last_modified)),
                content_type: content_type(m),
                content_encoding: m.content_encoding.clone(),
                content_disposition: m.content_disposition.clone(),
                content_language: m.content_language.clone(),
                cache_control: m.cache_control.clone(),
                expires: m.expires.clone(),
                metadata: m.user_metadata.clone(),
                parts_count: o.parts_count,
                tag_count: tag_count(&o.tags),
                version_id: versioned.then(|| v.id.clone()),
                checksum_type: ctype,
                expiration: self.expiration_header(&input.bucket, &input.key, o),
            },
            checksum
        );
        apply_ext!(out, o.ext.as_deref());
        if let Some(a) = self.archive_state(&input.bucket, &input.key, o) {
            out.restore = a.restore_header();
            out.archive_status = a.archive_status();
        }
        let mut resp = S3Response::new(out);
        if partial {
            resp.status = Some(http::StatusCode::PARTIAL_CONTENT);
        }
        Ok(resp)
    }

    async fn delete_object(&self, req: S3Request<DeleteObjectInput>) -> S3Result<S3Response<DeleteObjectOutput>> {
        let who = self.who(&req);
        let input = req.input;
        let b = self.bucket(&input.bucket)?;
        let mut objects = b.objects.write();
        if let Some(id) = &input.version_id {
            objext::check_version_id(id)?;
            objops::mfa_required(&b.cfg, input.mfa.as_deref())?;
            if objops::version_locked(&b, &objects, &input.key, id, self.clock.now(), input.bypass_governance_retention) {
                return Err(objext::locked_error());
            }
        }
        if let Some(c) = &input.if_match {
            match objects.get(&input.key).and_then(Entry::current) {
                Some(o) => check_conditions(o, Some(c), None, None, None)?,
                None => return Err(s3_error!(NoSuchKey)),
            }
        }
        let out = match input.version_id {
            Some(id) => {
                let marker = StoreBucket::delete_version(&mut objects, &input.key, &id);
                DeleteObjectOutput {
                    version_id: Some(id),
                    delete_marker: marker.filter(|m| *m),
                    ..Default::default()
                }
            }
            None => match b.delete_current(&mut objects, &input.key, self.clock.now(), who.user().unwrap_or(b.owner)) {
                Some(id) => DeleteObjectOutput {
                    version_id: Some(id),
                    delete_marker: Some(true),
                    ..Default::default()
                },
                None => DeleteObjectOutput::default(),
            },
        };
        Ok(S3Response::new(out))
    }

    async fn delete_objects(&self, req: S3Request<DeleteObjectsInput>) -> S3Result<S3Response<DeleteObjectsOutput>> {
        let who = self.who(&req);
        let b = self.bucket(&req.input.bucket)?;
        if req.input.delete.objects.len() > 1000 {
            return Err(s3_error!(
                MalformedXML,
                "The XML you provided was not well-formed or did not validate against our published schema"
            ));
        }
        let quiet = req.input.delete.quiet.unwrap_or(false);
        let mut deleted = Vec::new();
        let mut errors = Vec::new();
        let denied = req.extensions.get::<crate::access::Denied>();
        if req.input.delete.objects.iter().any(|o| o.version_id.is_some()) {
            objops::mfa_required(&b.cfg, req.input.mfa.as_deref())?;
        }
        {
            let mut objects = b.objects.write();
            for id in req.input.delete.objects {
                if let Some(d) = denied
                    && d.0.iter().any(|(k, v)| *k == id.key && *v == id.version_id)
                {
                    errors.push(Error {
                        code: Some("AccessDenied".into()),
                        key: Some(id.key),
                        message: Some("Access Denied".into()),
                        version_id: id.version_id,
                    });
                    continue;
                }
                let d = match id.version_id {
                    Some(vid) => {
                        if let Err(e) = objext::check_version_id(&vid) {
                            errors.push(delete_error(id.key, Some(vid), e));
                            continue;
                        }
                        if objops::version_locked(&b, &objects, &id.key, &vid, self.clock.now(), req.input.bypass_governance_retention) {
                            errors.push(delete_error(id.key, Some(vid), objext::locked_error()));
                            continue;
                        }
                        let marker = StoreBucket::delete_version(&mut objects, &id.key, &vid);
                        DeletedObject {
                            key: Some(id.key),
                            version_id: Some(vid.clone()),
                            delete_marker: marker.filter(|m| *m),
                            delete_marker_version_id: marker.filter(|m| *m).map(|_| vid),
                        }
                    }
                    None => match b.delete_current(&mut objects, &id.key, self.clock.now(), who.user().unwrap_or(b.owner)) {
                        Some(mid) => DeletedObject {
                            key: Some(id.key),
                            delete_marker: Some(true),
                            delete_marker_version_id: Some(mid),
                            ..Default::default()
                        },
                        None => DeletedObject {
                            key: Some(id.key),
                            ..Default::default()
                        },
                    },
                };
                if !quiet {
                    deleted.push(d);
                }
            }
        }
        Ok(S3Response::new(DeleteObjectsOutput {
            deleted: Some(deleted),
            errors: (!errors.is_empty()).then_some(errors),
            ..Default::default()
        }))
    }

    async fn copy_object(&self, req: S3Request<CopyObjectInput>) -> S3Result<S3Response<CopyObjectOutput>> {
        let who = self.who(&req);
        let input = req.input;
        let (src_v, src_versioned) = self.source_object(&input.copy_source)?;
        let src = obj(&src_v);
        objext::check_read_key(
            src.ext.as_deref(),
            input.copy_source_sse_customer_algorithm.as_deref(),
            input.copy_source_sse_customer_key.as_deref(),
            input.copy_source_sse_customer_key_md5.as_deref(),
        )?;
        self.require_copy_source_restored(&input.copy_source, src)?;
        check_conditions(
            src,
            input.copy_source_if_match.as_ref(),
            input.copy_source_if_none_match.as_ref(),
            input.copy_source_if_modified_since.as_ref(),
            input.copy_source_if_unmodified_since.as_ref(),
        )
        .map_err(|e| {
            if *e.code() == S3ErrorCode::NotModified {
                s3_error!(PreconditionFailed)
            } else {
                e
            }
        })?;
        let dst_bucket = self.bucket(&input.bucket)?;
        let owner = self.object_owner(&dst_bucket, who, input.acl.as_ref().map(|a| a.as_str()));
        let acl = acl::resolve(&dst_bucket.config, &acl::acl_args!(&input), false, self.parties(&dst_bucket, owner))?.map(Arc::new);
        let opts = object_opts!(input);
        let changes_attributes = !opts.is_empty();
        let mut ext = objext::resolve(opts, &dst_bucket, &self.clock)?;
        // A multipart object keeps answering GetObjectAttributes with its per-part checksums.
        if let Some(pc) = src.ext.as_ref().and_then(|e| e.part_checksums.clone()) {
            ext.get_or_insert_default().part_checksums = Some(pc);
        }
        let replace = input
            .metadata_directive
            .as_ref()
            .is_some_and(|d| d.as_str() == MetadataDirective::REPLACE);
        let replace_tags = input
            .tagging_directive
            .as_ref()
            .is_some_and(|d| d.as_str() == TaggingDirective::REPLACE);
        if let CopySource::Bucket {
            bucket,
            key,
            version_id: None,
        } = &input.copy_source
            && !replace
            && !changes_attributes
            && **bucket == *input.bucket
            && **key == *input.key
        {
            return Err(s3_error!(
                InvalidRequest,
                "This copy request is illegal because it is trying to copy an object to itself without changing the object's metadata, storage class, website redirect location or encryption attributes."
            ));
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
        let tags = if replace_tags {
            parse_tagging_header(input.tagging.as_deref())?
        } else {
            src.tags.clone()
        };
        let now = self.clock.now();
        let e_tag = objext::copy_etag(src, ext.as_ref());
        let ext = objext::finish(ext, &dst_bucket, now);
        let new = StoreObject {
            data: src.data.clone(),
            meta,
            e_tag: e_tag.clone(),
            last_modified: now,
            checksum: src.checksum.clone(),
            checksum_type: src.checksum_type.clone(),
            parts_count: src.parts_count,
            part_sizes: src.part_sizes.clone(),
            tags,
            acl,
            ext: ext.clone(),
        };
        let version_id = {
            let mut objects = dst_bucket.objects.write();
            dst_bucket.put(&mut objects, input.key, new, owner)
        };
        let mut out = CopyObjectOutput {
            copy_object_result: Some(CopyObjectResult {
                e_tag: Some(ETag::Strong(e_tag)),
                last_modified: Some(ts(now)),
                ..Default::default()
            }),
            version_id,
            copy_source_version_id: src_versioned.then(|| src_v.id.clone()),
            ..Default::default()
        };
        apply_sse!(out, objext::sse_out(ext.as_deref()));
        out.ssekms_encryption_context = objext::sse_context(ext.as_deref());
        Ok(S3Response::new(out))
    }

    async fn put_object_tagging(&self, req: S3Request<PutObjectTaggingInput>) -> S3Result<S3Response<PutObjectTaggingOutput>> {
        let input = req.input;
        let tags = tags_from_set(input.tagging.tag_set)?;
        let b = self.bucket(&input.bucket)?;
        let versioned = *b.versioning.read() != Versioning::Unversioned;
        let mut objects = b.objects.write();
        let entry = objects.get_mut(&input.key).ok_or_else(|| s3_error!(NoSuchKey))?;
        let pos = match &input.version_id {
            Some(id) => entry
                .versions
                .iter()
                .rposition(|v| &v.id == id)
                .ok_or_else(|| s3_error!(NoSuchVersion))?,
            None => entry.versions.len() - 1,
        };
        let v = &entry.versions[pos];
        let Some(o) = &v.obj else {
            return Err(if input.version_id.is_some() {
                s3_error!(MethodNotAllowed)
            } else {
                s3_error!(NoSuchKey)
            });
        };
        let mut o = o.clone();
        o.tags = tags;
        let id = v.id.clone();
        entry.versions[pos] = Arc::new(Version {
            id: id.clone(),
            obj: Some(o),
            last_modified: v.last_modified,
            owner: v.owner,
        });
        Ok(S3Response::new(PutObjectTaggingOutput {
            version_id: versioned.then_some(id),
        }))
    }

    async fn get_object_tagging(&self, req: S3Request<GetObjectTaggingInput>) -> S3Result<S3Response<GetObjectTaggingOutput>> {
        let input = req.input;
        let (v, versioned) = self.version(&input.bucket, &input.key, input.version_id.as_deref())?;
        Ok(S3Response::new(GetObjectTaggingOutput {
            tag_set: tag_set(&obj(&v).tags),
            version_id: versioned.then(|| v.id.clone()),
        }))
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
        Ok(S3Response::new(DeleteObjectTaggingOutput {
            version_id: out.output.version_id,
        }))
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
        if self.fast_list
            && let Some(body) = self.render_v2(&b, &input, &prefix, max_keys, after)
        {
            return Ok(pre_rendered(ListObjectsV2Output::default(), body));
        }
        let owners = (input.fetch_owner == Some(true)).then_some(&*self.users);
        let l = list(&b.objects.read(), &prefix, input.delimiter.as_deref(), after, max_keys, owners);
        let key_count = (l.contents.len() + l.prefixes.len()) as i32;
        let mut out = ListObjectsV2Output {
            name: Some(input.bucket),
            prefix: Some(prefix),
            delimiter: input.delimiter.filter(|d| !d.is_empty()),
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
        if input
            .optional_object_attributes
            .iter()
            .flatten()
            .any(|a| a.as_str() == OptionalObjectAttributes::RESTORE_STATUS)
            && let Some(contents) = &mut out.contents
        {
            self.add_restore_status(&b, contents);
        }
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
        if self.fast_list
            && let Some(body) = self.render_v1(&b, &input, &prefix, max_keys, input.marker.as_deref())
        {
            return Ok(pre_rendered(ListObjectsOutput::default(), body));
        }
        let l = list(
            &b.objects.read(),
            &prefix,
            input.delimiter.as_deref(),
            input.marker.as_deref(),
            max_keys,
            // ListObjects (v1) returns the owner of every key.
            Some(&self.users),
        );
        let next_marker = if input.delimiter.as_deref().is_some_and(|d| !d.is_empty()) {
            l.next
        } else {
            None
        };
        let mut out = ListObjectsOutput {
            name: Some(input.bucket),
            prefix: Some(prefix),
            delimiter: input.delimiter.filter(|d| !d.is_empty()),
            // Always echoed, empty when the request had none.
            marker: Some(input.marker.unwrap_or_default()),
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
        if input.max_keys.is_some_and(|n| n < 0) {
            return Err(s3_error!(
                InvalidArgument,
                "Argument maxKeys must be an integer between 0 and 2147483647"
            ));
        }
        if input.version_id_marker.as_deref().is_some_and(|v| !v.is_empty()) && input.key_marker.as_deref().is_none_or(str::is_empty) {
            return Err(s3_error!(
                InvalidArgument,
                "A version-id marker cannot be specified without a key marker."
            ));
        }
        let max_keys = input.max_keys.unwrap_or(1000).clamp(0, 1000) as usize;
        if self.fast_list
            && let Some(body) = self.render_versions(&b, &input, &prefix, max_keys)
        {
            return Ok(pre_rendered(ListObjectVersionsOutput::default(), body));
        }
        let mut versions = Vec::new();
        let mut markers = Vec::new();
        let mut prefixes = Vec::new();
        let end = walk_versions(
            &b.objects.read(),
            &input,
            &prefix,
            max_keys,
            |key, v, is_latest| {
                let is_latest = Some(is_latest);
                match &v.obj {
                    Some(o) => versions.push(ObjectVersion {
                        key: Some(key.to_owned()),
                        version_id: Some(v.id.clone()),
                        is_latest,
                        e_tag: Some(ETag::Strong(o.e_tag.clone())),
                        last_modified: Some(ts(o.last_modified)),
                        size: Some(o.data.size as i64),
                        storage_class: Some(match objext::storage_class(o.ext.as_deref()) {
                            Some(c) => ObjectVersionStorageClass::from(c.as_str().to_owned()),
                            None => ObjectVersionStorageClass::from_static(ObjectVersionStorageClass::STANDARD),
                        }),
                        owner: Some(self.users.owner(v.owner)),
                        ..Default::default()
                    }),
                    None => markers.push(DeleteMarkerEntry {
                        key: Some(key.to_owned()),
                        version_id: Some(v.id.clone()),
                        is_latest,
                        last_modified: Some(ts(v.last_modified)),
                        owner: Some(self.users.owner(v.owner)),
                    }),
                }
            },
            |p| {
                prefixes.push(CommonPrefix {
                    prefix: Some(p.to_owned()),
                });
            },
        );
        let mut out = ListObjectVersionsOutput {
            name: Some(input.bucket),
            prefix: Some(prefix),
            delimiter: input.delimiter.filter(|d| !d.is_empty()),
            // S3 always writes the (possibly empty) markers.
            key_marker: Some(input.key_marker.unwrap_or_default()),
            version_id_marker: Some(input.version_id_marker.unwrap_or_default()),
            max_keys: Some(max_keys as i32),
            is_truncated: Some(end.truncated),
            next_key_marker: end.next_key,
            next_version_id_marker: end.next_version_id,
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

    async fn create_multipart_upload(
        &self,
        req: S3Request<CreateMultipartUploadInput>,
    ) -> S3Result<S3Response<CreateMultipartUploadOutput>> {
        let who = self.who(&req);
        let input = req.input;
        let b = self.bucket(&input.bucket)?;
        let tags = parse_tagging_header(input.tagging.as_deref())?;
        let ty = upload_checksum_type(input.checksum_algorithm.as_ref(), input.checksum_type.as_ref())?;
        let owner = self.object_owner(&b, who, input.acl.as_ref().map(|a| a.as_str()));
        let acl = acl::resolve(&b.config, &acl::acl_args!(&input), false, self.parties(&b, owner))?.map(Arc::new);
        let ext = objext::resolve(object_opts!(input), &b, &self.clock)?;
        let upload_id = uuid::Uuid::new_v4().simple().to_string();
        b.uploads.write().insert(
            (input.key.clone(), upload_id.clone()),
            Upload {
                initiator: who.user().unwrap_or(b.owner),
                owner,
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
                ext: ext.clone(),
                acl,
                initiated: self.clock.now(),
                checksum_algorithm: input.checksum_algorithm.clone(),
                checksum_type: ty.clone(),
                parts: BTreeMap::new(),
            },
        );
        let mut out = CreateMultipartUploadOutput {
            bucket: Some(input.bucket),
            key: Some(input.key),
            upload_id: Some(upload_id),
            checksum_type: input.checksum_algorithm.is_some().then_some(ty),
            checksum_algorithm: input.checksum_algorithm,
            ssekms_encryption_context: objext::sse_context(ext.as_ref()),
            ..Default::default()
        };
        apply_sse!(out, objext::sse_out(ext.as_ref()));
        Ok(S3Response::new(out))
    }

    async fn upload_part(&self, req: S3Request<UploadPartInput>) -> S3Result<S3Response<UploadPartOutput>> {
        let input = req.input;
        let b = self.bucket(&input.bucket)?;
        if !(1..=10000).contains(&input.part_number) {
            return Err(s3_error!(
                InvalidArgument,
                "Part number must be an integer between 1 and 10000, inclusive"
            ));
        }
        let id = (input.key.clone(), input.upload_id.clone());
        let (algorithm, sse) = {
            let uploads = b.uploads.read();
            let up = uploads.get(&id).ok_or_else(|| s3_error!(NoSuchUpload))?;
            // An SSE-C upload takes its key with every part.
            objext::check_part_key(
                up.ext.as_ref(),
                input.sse_customer_algorithm.as_deref(),
                input.sse_customer_key.as_deref(),
                input.sse_customer_key_md5.as_deref(),
            )?;
            // Without an algorithm, S3 gives the object a full-object CRC64NVME, so each part needs one.
            let algorithm = input
                .checksum_algorithm
                .clone()
                .or_else(|| up.checksum_algorithm.clone())
                .unwrap_or_else(|| ChecksumAlgorithm::from_static(ChecksumAlgorithm::CRC64NVME));
            (algorithm, objext::sse_out(up.ext.as_ref()))
        };
        let expected = checksum_from!(&input);
        let ing = ingest(
            self.sink(&req.headers),
            input.body,
            input.content_length,
            input.content_md5.as_deref(),
            expected,
            Some(&algorithm),
            Trailers::of(req.trailing_headers, &req.extensions),
        )
        .await?;
        let mut uploads = b.uploads.write();
        let up = uploads.get_mut(&id).ok_or_else(|| s3_error!(NoSuchUpload))?;
        up.parts.insert(
            input.part_number,
            Part {
                data: ObjectData::from_segments(vec![ing.data]),
                md5: ing.md5,
                last_modified: self.clock.now(),
                checksum: ing.checksum.clone(),
            },
        );
        let mut out = with_checksum!(
            UploadPartOutput {
                e_tag: Some(ETag::Strong(hex::encode(ing.md5)))
            },
            ing.checksum
        );
        apply_sse!(out, sse);
        Ok(S3Response::new(out))
    }

    async fn upload_part_copy(&self, req: S3Request<UploadPartCopyInput>) -> S3Result<S3Response<UploadPartCopyOutput>> {
        let input = req.input;
        let b = self.bucket(&input.bucket)?;
        if !(1..=10000).contains(&input.part_number) {
            return Err(s3_error!(
                InvalidArgument,
                "Part number must be an integer between 1 and 10000, inclusive"
            ));
        }
        let (src_v, src_versioned) = self.source_object(&input.copy_source)?;
        let src = obj(&src_v);
        objext::check_read_key(
            src.ext.as_deref(),
            input.copy_source_sse_customer_algorithm.as_deref(),
            input.copy_source_sse_customer_key.as_deref(),
            input.copy_source_sse_customer_key_md5.as_deref(),
        )?;
        self.require_copy_source_restored(&input.copy_source, src)?;
        check_conditions(
            src,
            input.copy_source_if_match.as_ref(),
            input.copy_source_if_none_match.as_ref(),
            input.copy_source_if_modified_since.as_ref(),
            input.copy_source_if_unmodified_since.as_ref(),
        )
        .map_err(|e| {
            if *e.code() == S3ErrorCode::NotModified {
                s3_error!(PreconditionFailed)
            } else {
                e
            }
        })?;
        let range = match &input.copy_source_range {
            Some(r) => parse_copy_range(r, src.data.size)?,
            None => 0..src.data.size,
        };
        let id = (input.key.clone(), input.upload_id.clone());
        let (algorithm, sse) = {
            let uploads = b.uploads.read();
            let up = uploads.get(&id).ok_or_else(|| s3_error!(NoSuchUpload))?;
            objext::check_part_key(
                up.ext.as_ref(),
                input.sse_customer_algorithm.as_deref(),
                input.sse_customer_key.as_deref(),
                input.sse_customer_key_md5.as_deref(),
            )?;
            let algorithm = up
                .checksum_algorithm
                .clone()
                .unwrap_or_else(|| ChecksumAlgorithm::from_static(ChecksumAlgorithm::CRC64NVME));
            (algorithm, objext::sse_out(up.ext.as_ref()))
        };
        // The part shares the source's buffers; only its digests have to be computed.
        let segments = src.data.slice(range);
        let (md5, checksum) = digest_segments(&segments, Some(&algorithm))?;
        let now = self.clock.now();
        let mut uploads = b.uploads.write();
        let up = uploads.get_mut(&id).ok_or_else(|| s3_error!(NoSuchUpload))?;
        up.parts.insert(
            input.part_number,
            Part {
                data: ObjectData::from_segments(segments),
                md5,
                last_modified: now,
                checksum: checksum.clone(),
            },
        );
        let mut out = UploadPartCopyOutput {
            copy_part_result: Some(with_checksum!(
                CopyPartResult {
                    e_tag: Some(ETag::Strong(hex::encode(md5))),
                    last_modified: Some(ts(now))
                },
                checksum
            )),
            copy_source_version_id: src_versioned.then(|| src_v.id.clone()),
            ..Default::default()
        };
        apply_sse!(out, sse);
        Ok(S3Response::new(out))
    }

    async fn list_parts(&self, req: S3Request<ListPartsInput>) -> S3Result<S3Response<ListPartsOutput>> {
        let input = req.input;
        let b = self.bucket(&input.bucket)?;
        let uploads = b.uploads.read();
        let up = uploads
            .get(&(input.key.clone(), input.upload_id.clone()))
            .ok_or_else(|| s3_error!(NoSuchUpload))?;
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
        let next = if truncated {
            parts.last().and_then(|p| p.part_number)
        } else {
            None
        };
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
            checksum_type: up.checksum_algorithm.is_some().then(|| up.checksum_type.clone()),
            owner: Some(self.users.owner(up.owner)),
            initiator: Some(self.initiator(up.initiator)),
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
            if let Some(d) = delimiter
                && let Some(pos) = key[prefix.len()..].find(d)
            {
                let cp = key[..prefix.len() + pos + d.len()].to_owned();
                if prefixes.last().and_then(|p| p.prefix.as_ref()) != Some(&cp) {
                    prefixes.push(CommonPrefix { prefix: Some(cp) });
                }
                last = Some((key.clone(), id.clone()));
                continue;
            }
            uploads_out.push(MultipartUpload {
                key: Some(key.clone()),
                upload_id: Some(id.clone()),
                initiated: Some(ts(up.initiated)),
                storage_class: Some(StorageClass::from_static(StorageClass::STANDARD)),
                owner: Some(self.users.owner(up.owner)),
                initiator: Some(self.initiator(up.initiator)),
                checksum_algorithm: up.checksum_algorithm.clone(),
                checksum_type: up.checksum_algorithm.is_some().then(|| up.checksum_type.clone()),
            });
            last = Some((key.clone(), id.clone()));
        }
        let (next_key, next_id) = if truncated {
            last.map(|(k, i)| (Some(k), Some(i))).unwrap_or_default()
        } else {
            (None, None)
        };
        let mut out = ListMultipartUploadsOutput {
            bucket: Some(input.bucket),
            prefix: Some(prefix),
            delimiter: input.delimiter.filter(|d| !d.is_empty()),
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

    async fn complete_multipart_upload(
        &self,
        req: S3Request<CompleteMultipartUploadInput>,
    ) -> S3Result<S3Response<CompleteMultipartUploadOutput>> {
        let input = req.input;
        let b = self.bucket(&input.bucket)?;
        let claimed_whole = checksum_from!(&input);
        let requested = input.multipart_upload.and_then(|m| m.parts).unwrap_or_default();
        if requested.is_empty() {
            return Err(s3_error!(
                MalformedXML,
                "The XML you provided was not well-formed or did not validate against our published schema"
            ));
        }
        let id = (input.key.clone(), input.upload_id.clone());
        let (obj, owner, checksum) = {
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
                if let Some(e) = &cp.e_tag
                    && e.value().trim_matches('"') != hex::encode(p.md5)
                {
                    return Err(s3_error!(InvalidPart));
                }
                let claimed = checksum_from!(cp);
                for (i, (name, _)) in ALGS.iter().enumerate() {
                    if let (Some(want), Some(have)) = (slot(&claimed, i), slot(&p.checksum, i))
                        && want != have
                    {
                        return Err(s3_error!(InvalidPart, "The {name} checksum for part {n} did not match"));
                    }
                }
                if i + 1 < requested.len() && p.data.size < 5 * 1024 * 1024 {
                    return Err(s3_error!(
                        EntityTooSmall,
                        "Your proposed upload is smaller than the minimum allowed object size."
                    ));
                }
                segments.extend(p.data.segments.iter().cloned());
                sizes.push(p.data.size);
                md5s.push(p.md5);
                checksums.push((&p.checksum, p.data.size));
            }
            let alg_idx = match &up.checksum_algorithm {
                Some(a) => alg_index(a)?,
                None => CRC64NVME,
            };
            let checksum = multipart_checksum(alg_idx, &up.checksum_type, &checksums);
            // A request-level checksum (the full-object value, or the composite `<digest>-<parts>`)
            // must match ours; anything else, a malformed value included, is a BadDigest.
            if let (Some(want), Some(have)) = (slot(&claimed_whole, alg_idx), slot(&checksum, alg_idx))
                && want != have
            {
                return Err(s3_error!(
                    BadDigest,
                    "The {} you specified did not match the calculated checksum.",
                    ALGS[alg_idx].1
                ));
            }
            let has_checksum = slot(&checksum, alg_idx).is_some();
            let mut ext = up.ext.clone();
            ext.get_or_insert_default().part_checksums = Some(checksums.iter().map(|(c, _)| (*c).clone()).collect());
            let now = self.clock.now();
            let ext = objext::finish(ext, &b, now);
            let obj = StoreObject {
                data: ObjectData::from_segments(segments),
                meta: up.meta.clone(),
                e_tag: multipart_etag(&md5s),
                last_modified: now,
                checksum: checksum.clone(),
                checksum_type: has_checksum.then(|| up.checksum_type.clone()),
                parts_count: Some(md5s.len() as i32),
                part_sizes: Some(sizes.into()),
                tags: up.tags.clone(),
                ext,
                acl: up.acl.clone(),
            };
            (obj, up.owner, checksum)
        };
        let ctype = obj.checksum_type.clone();
        let e_tag = obj.e_tag.clone();
        let sse = objext::sse_out(obj.ext.as_deref());
        let version_id = {
            let mut objects = b.objects.write();
            check_write_conditions(objects.get(&input.key), input.if_match.as_ref(), input.if_none_match.as_ref())?;
            b.put(&mut objects, input.key.clone(), obj, owner)
        };
        b.uploads.write().remove(&id);
        let mut out = with_checksum!(
            CompleteMultipartUploadOutput {
                bucket: Some(input.bucket.clone()),
                key: Some(input.key.clone()),
                location: Some(format!("/{}/{}", input.bucket, input.key)),
                e_tag: Some(ETag::Strong(e_tag)),
                version_id: version_id,
                checksum_type: ctype,
            },
            checksum
        );
        out.server_side_encryption = sse.algorithm;
        out.ssekms_key_id = sse.kms_key_id;
        out.bucket_key_enabled = sse.bucket_key;
        Ok(S3Response::new(out))
    }

    async fn put_object_lock_configuration(
        &self,
        req: S3Request<PutObjectLockConfigurationInput>,
    ) -> S3Result<S3Response<PutObjectLockConfigurationOutput>> {
        self.do_put_object_lock_configuration(req).await
    }

    async fn get_object_lock_configuration(
        &self,
        req: S3Request<GetObjectLockConfigurationInput>,
    ) -> S3Result<S3Response<GetObjectLockConfigurationOutput>> {
        self.do_get_object_lock_configuration(req).await
    }

    async fn put_object_retention(&self, req: S3Request<PutObjectRetentionInput>) -> S3Result<S3Response<PutObjectRetentionOutput>> {
        self.do_put_object_retention(req).await
    }

    async fn get_object_retention(&self, req: S3Request<GetObjectRetentionInput>) -> S3Result<S3Response<GetObjectRetentionOutput>> {
        self.do_get_object_retention(req).await
    }

    async fn put_object_legal_hold(&self, req: S3Request<PutObjectLegalHoldInput>) -> S3Result<S3Response<PutObjectLegalHoldOutput>> {
        self.do_put_object_legal_hold(req).await
    }

    async fn get_object_legal_hold(&self, req: S3Request<GetObjectLegalHoldInput>) -> S3Result<S3Response<GetObjectLegalHoldOutput>> {
        self.do_get_object_legal_hold(req).await
    }

    async fn put_bucket_encryption(&self, req: S3Request<PutBucketEncryptionInput>) -> S3Result<S3Response<PutBucketEncryptionOutput>> {
        self.do_put_bucket_encryption(req).await
    }

    async fn get_bucket_encryption(&self, req: S3Request<GetBucketEncryptionInput>) -> S3Result<S3Response<GetBucketEncryptionOutput>> {
        self.do_get_bucket_encryption(req).await
    }

    async fn delete_bucket_encryption(
        &self,
        req: S3Request<DeleteBucketEncryptionInput>,
    ) -> S3Result<S3Response<DeleteBucketEncryptionOutput>> {
        self.do_delete_bucket_encryption(req).await
    }

    async fn select_object_content(&self, req: S3Request<SelectObjectContentInput>) -> S3Result<S3Response<SelectObjectContentOutput>> {
        self.do_select_object_content(req).await
    }

    async fn get_object_attributes(&self, req: S3Request<GetObjectAttributesInput>) -> S3Result<S3Response<GetObjectAttributesOutput>> {
        self.do_get_object_attributes(req).await
    }

    async fn restore_object(&self, req: S3Request<RestoreObjectInput>) -> S3Result<S3Response<RestoreObjectOutput>> {
        self.do_restore_object(req).await
    }

    async fn put_bucket_intelligent_tiering_configuration(
        &self,
        req: S3Request<PutBucketIntelligentTieringConfigurationInput>,
    ) -> S3Result<S3Response<PutBucketIntelligentTieringConfigurationOutput>> {
        self.do_put_it_config(req)
    }

    async fn get_bucket_intelligent_tiering_configuration(
        &self,
        req: S3Request<GetBucketIntelligentTieringConfigurationInput>,
    ) -> S3Result<S3Response<GetBucketIntelligentTieringConfigurationOutput>> {
        self.do_get_it_config(req)
    }

    async fn list_bucket_intelligent_tiering_configurations(
        &self,
        req: S3Request<ListBucketIntelligentTieringConfigurationsInput>,
    ) -> S3Result<S3Response<ListBucketIntelligentTieringConfigurationsOutput>> {
        self.do_list_it_configs(req)
    }

    async fn delete_bucket_intelligent_tiering_configuration(
        &self,
        req: S3Request<DeleteBucketIntelligentTieringConfigurationInput>,
    ) -> S3Result<S3Response<DeleteBucketIntelligentTieringConfigurationOutput>> {
        self.do_delete_it_config(req)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn crc_of(alg_idx: usize, data: &[u8]) -> Checksum {
        let mut h = ChecksumHasher::default();
        enable(&mut h, alg_idx);
        h.update(data);
        h.finalize()
    }

    #[test]
    fn full_object_checksum_matches_crc_of_whole_object() {
        let data: Vec<u8> = (0..3_000_000u32).map(|i| i.wrapping_mul(2_654_435_761) as u8).collect();
        let cuts = [0, 1_048_576, 2_500_000, data.len()];
        for alg in [CRC32, CRC32C, CRC64NVME] {
            let parts: Vec<Checksum> = cuts.windows(2).map(|w| crc_of(alg, &data[w[0]..w[1]])).collect();
            let refs: Vec<(&Checksum, u64)> = parts.iter().zip(cuts.windows(2)).map(|(c, w)| (c, (w[1] - w[0]) as u64)).collect();
            let got = multipart_checksum(alg, &checksum_type(ChecksumType::FULL_OBJECT), &refs);
            assert_eq!(slot(&got, alg), slot(&crc_of(alg, &data), alg), "{}", ALGS[alg].0);
        }
    }

    #[test]
    fn composite_checksum_hashes_part_digests() {
        let a = crc_of(3, b"hello");
        let b = crc_of(3, b"world");
        let got = multipart_checksum(3, &checksum_type(ChecksumType::COMPOSITE), &[(&a, 5), (&b, 5)]);
        let mut raw = base64_simd::STANDARD.decode_to_vec(a.checksum_sha256.unwrap()).unwrap();
        raw.extend(base64_simd::STANDARD.decode_to_vec(b.checksum_sha256.unwrap()).unwrap());
        let want = format!("{}-2", crc_of(3, &raw).checksum_sha256.unwrap());
        assert_eq!(got.checksum_sha256.as_deref(), Some(want.as_str()));
    }

    #[test]
    fn missing_part_checksum_yields_none() {
        let a = crc_of(CRC32, b"x");
        let got = multipart_checksum(
            CRC32,
            &checksum_type(ChecksumType::FULL_OBJECT),
            &[(&a, 1), (&Checksum::default(), 1)],
        );
        assert!(got.checksum_crc32.is_none());
    }

    #[test]
    fn checksum_type_defaults_and_rejections() {
        let alg = |s: &'static str| ChecksumAlgorithm::from_static(s);
        let ty = |a: Option<&ChecksumAlgorithm>, t: Option<&'static str>| {
            upload_checksum_type(a, t.map(checksum_type).as_ref()).map(|t| t.as_str().to_owned())
        };
        assert_eq!(ty(None, None).unwrap(), "FULL_OBJECT");
        assert_eq!(ty(Some(&alg("CRC32")), None).unwrap(), "COMPOSITE");
        assert_eq!(ty(Some(&alg("crc32c")), Some("FULL_OBJECT")).unwrap(), "FULL_OBJECT");
        assert_eq!(ty(Some(&alg("CRC64NVME")), None).unwrap(), "FULL_OBJECT");
        assert!(ty(Some(&alg("SHA256")), Some("FULL_OBJECT")).is_err());
        assert!(ty(Some(&alg("CRC64NVME")), Some("COMPOSITE")).is_err());
    }

    #[test]
    fn copy_range() {
        assert_eq!(parse_copy_range("bytes=0-9", 10).unwrap(), 0..10);
        assert!(parse_copy_range("bytes=0-10", 10).is_err());
        assert!(parse_copy_range("bytes=-5", 10).is_err());
        assert!(parse_copy_range("bytes=5-", 10).is_err());
        assert!(parse_copy_range("garbage", 10).is_err());
    }

    #[test]
    fn url_encoding_round_trips_through_percent_and_form_decoding() {
        let key = "sp ace/üñí©ødé/a+b=c&d?e#f.txt";
        let enc = url_encode(key);
        assert!(enc.contains("%2B") && !enc.contains(' '));
        assert!(enc.starts_with("sp%20ace/"), "{enc}");
        assert!(!enc.contains('+'), "{enc}");
        let dec: String = form_urlencoded::parse(format!("k={enc}").as_bytes()).next().unwrap().1.into_owned();
        assert_eq!(dec, key);
        let pct = percent_encoding::percent_decode_str(&enc).decode_utf8().unwrap();
        assert_eq!(pct, key);
    }

    fn entry(versions: Vec<Option<&str>>) -> Entry {
        let now = SystemTime::now();
        Entry {
            versions: versions
                .into_iter()
                .enumerate()
                .map(|(i, v)| {
                    Arc::new(Version {
                        id: i.to_string(),
                        obj: v.map(|e| StoreObject {
                            data: ObjectData::default(),
                            meta: ObjectMeta::default(),
                            e_tag: e.to_owned(),
                            last_modified: now,
                            checksum: Checksum::default(),
                            checksum_type: None,
                            parts_count: None,
                            part_sizes: None,
                            tags: Vec::new(),
                            ext: None,
                            acl: None,
                        }),
                        last_modified: now,
                        owner: UserId::DEFAULT,
                    })
                })
                .collect(),
        }
    }

    #[test]
    fn listing_skips_delete_markers_and_their_prefixes() {
        let mut m = BTreeMap::new();
        m.insert("a/1".to_owned(), entry(vec![Some("x"), None]));
        m.insert("b/1".to_owned(), entry(vec![None, Some("y")]));
        m.insert("c".to_owned(), entry(vec![None]));
        m.insert("d".to_owned(), entry(vec![Some("z")]));
        let l = list(&m, "", Some("/"), None, 1000, None);
        let prefixes: Vec<_> = l.prefixes.iter().map(|p| p.prefix.clone().unwrap()).collect();
        let keys: Vec<_> = l.contents.iter().map(|o| o.key.clone().unwrap()).collect();
        assert_eq!(prefixes, ["b/"]);
        assert_eq!(keys, ["d"]);

        let page = list(&m, "", None, None, 1, None);
        assert!(page.truncated);
        assert_eq!(page.contents[0].key.as_deref(), Some("b/1"));
        let rest = list(&m, "", None, page.next.as_deref(), 1, None);
        assert_eq!(rest.contents[0].key.as_deref(), Some("d"));
        assert!(!rest.truncated);
    }

    #[test]
    fn a_marker_that_is_a_common_prefix_skips_everything_under_it() {
        let mut m = BTreeMap::new();
        for k in ["asdf", "boo/bar", "boo/baz/xyzzy", "cquux/bla", "cquux/thud"] {
            m.insert(k.to_owned(), entry(vec![Some("x")]));
        }
        let page = |prefix: &str, after: Option<&str>, max| {
            let l = list(&m, prefix, Some("/"), after, max, None);
            let keys: Vec<_> = l.contents.iter().map(|o| o.key.clone().unwrap()).collect();
            let prefixes: Vec<_> = l.prefixes.iter().map(|p| p.prefix.clone().unwrap()).collect();
            (keys, prefixes, l.truncated, l.next)
        };
        let s = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(page("", None, 1), (s(&["asdf"]), s(&[]), true, Some("asdf".into())));
        assert_eq!(page("", Some("asdf"), 1), (s(&[]), s(&["boo/"]), true, Some("boo/".into())));
        // Resuming at "boo/" does not list "boo/" again, and the last page is not truncated.
        assert_eq!(page("", Some("boo/"), 1), (s(&[]), s(&["cquux/"]), false, None));
        assert_eq!(page("", Some("boo/"), 2), (s(&[]), s(&["cquux/"]), false, None));
        // Under a prefix, the marker "boo/baz/" is itself a common prefix of the listing.
        assert_eq!(page("boo/", None, 1), (s(&["boo/bar"]), s(&[]), true, Some("boo/bar".into())));
        assert_eq!(page("boo/", Some("boo/bar"), 1), (s(&[]), s(&["boo/baz/"]), false, None));
        assert_eq!(page("boo/", Some("boo/baz/"), 1), (s(&[]), s(&[]), false, None));
        // A marker inside a prefix (not itself a prefix) still lists the prefix.
        assert_eq!(page("", Some("boo/bar"), 5), (s(&[]), s(&["boo/", "cquux/"]), false, None));
    }

    #[test]
    fn content_encoding_keeps_the_clients_spelling_minus_aws_chunked() {
        let f = |v: &str| strip_aws_chunked(Some(v.to_owned()));
        assert_eq!(f("deflate, gzip").as_deref(), Some("deflate, gzip"));
        assert_eq!(f("deflate,gzip").as_deref(), Some("deflate,gzip"));
        assert_eq!(f("gzip, aws-chunked").as_deref(), Some("gzip"));
        assert_eq!(f("aws-chunked, gzip").as_deref(), Some("gzip"));
        assert_eq!(f("deflate, AWS-Chunked, gzip").as_deref(), Some("deflate, gzip"));
        assert_eq!(f("aws-chunked"), None);
        assert_eq!(f("aws-chunked, aws-chunked"), None);
        assert_eq!(strip_aws_chunked(None), None);
    }
}
