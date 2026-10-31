//! Object attributes most objects never use: storage class, website redirect, server-side
//! encryption, Object Lock. They live in an optional [`ObjectExt`] on the object, so a plain PUT
//! allocates nothing and reads one `Option` and one atomic; buckets only pay for what they
//! configured ([`BucketCfg`]).
//!
//! Object data is never encrypted: SSE only has to look right to the client, so SSE-C keeps the
//! MD5 of the key instead of the key.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use parking_lot::RwLock;
use s3s::dto::*;
use s3s::{S3Error, S3Result, s3_error};

use crate::clock::Clock;
use crate::store::{Bucket, Object};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LockMode {
    Governance,
    Compliance,
}

impl LockMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "GOVERNANCE" => Some(Self::Governance),
            "COMPLIANCE" => Some(Self::Compliance),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Governance => ObjectLockMode::GOVERNANCE,
            Self::Compliance => ObjectLockMode::COMPLIANCE,
        }
    }
}

#[derive(Clone)]
pub struct Retention {
    pub mode: LockMode,
    pub until: Timestamp,
}

impl Retention {
    fn active(&self, now: &Timestamp) -> bool {
        self.until > *now
    }
}

#[derive(Clone, PartialEq)]
pub enum Sse {
    Kms {
        dsse: bool,
        key_id: Option<String>,
        context: Option<String>,
        bucket_key: Option<bool>,
    },
    /// SSE-C: the base64 MD5 of the customer key.
    Customer { key_md5: String },
}

#[derive(Clone, Default)]
pub struct ObjectExt {
    /// `None` is STANDARD.
    pub storage_class: Option<StorageClass>,
    pub website_redirect: Option<String>,
    /// `None` is SSE-S3, which every S3 object has by default.
    pub sse: Option<Sse>,
    pub retention: Option<Retention>,
    /// `None` until a legal hold was ever set; `Some(false)` is `OFF`.
    pub legal_hold: Option<bool>,
    /// Per-part checksums of a multipart object, for GetObjectAttributes.
    pub part_checksums: Option<Arc<[Checksum]>>,
}

impl ObjectExt {
    fn is_plain(&self) -> bool {
        self.storage_class.is_none()
            && self.website_redirect.is_none()
            && self.sse.is_none()
            && self.retention.is_none()
            && self.legal_hold.is_none()
            && self.part_checksums.is_none()
    }

    pub fn customer_key_md5(&self) -> Option<&str> {
        match &self.sse {
            Some(Sse::Customer { key_md5 }) => Some(key_md5),
            _ => None,
        }
    }

    /// Whether DeleteObject of this version must be refused right now.
    pub fn blocks_delete(&self, now: &Timestamp, bypass_governance: bool) -> bool {
        if self.legal_hold == Some(true) {
            return true;
        }
        match &self.retention {
            Some(r) if r.active(now) => r.mode == LockMode::Compliance || !bypass_governance,
            _ => false,
        }
    }
}

/// Whether a version may be permanently deleted. Only buckets with Object Lock call this.
pub fn delete_blocked(o: &Object, now: SystemTime, bypass_governance: bool) -> bool {
    o.ext
        .as_ref()
        .is_some_and(|e| e.blocks_delete(&Timestamp::from(now), bypass_governance))
}

pub fn locked_error() -> S3Error {
    s3_error!(AccessDenied, "Access Denied because object protected by object lock.")
}

pub fn missing_lock_config() -> S3Error {
    s3_error!(InvalidRequest, "Bucket is missing Object Lock Configuration")
}

/// Whether replacing `old` retention with `new` (`None` clears it) is allowed.
pub fn check_retention_change(old: Option<&Retention>, new: Option<&Retention>, now: &Timestamp, bypass: bool) -> S3Result<()> {
    let Some(old) = old.filter(|r| r.active(now)) else { return Ok(()) };
    let not_weaker = new.is_some_and(|n| (n.mode == LockMode::Compliance || old.mode == LockMode::Governance) && n.until >= old.until);
    if not_weaker || (old.mode == LockMode::Governance && bypass) {
        Ok(())
    } else {
        Err(locked_error())
    }
}

#[derive(Clone, Copy)]
enum Period {
    Days(i32),
    Years(i32),
}

struct DefaultRetention {
    mode: LockMode,
    period: Period,
}

#[derive(Clone)]
pub struct EncryptionRule {
    pub algorithm: ServerSideEncryption,
    pub kms_key_id: Option<String>,
    pub bucket_key: Option<bool>,
    pub blocked: Option<BlockedEncryptionTypes>,
}

#[derive(Default)]
struct CfgInner {
    lock_enabled: bool,
    retention: Option<DefaultRetention>,
    encryption: Option<EncryptionRule>,
    mfa_delete: Option<bool>,
}

/// Bucket settings that change what new objects look like or what may be done to them.
#[derive(Default)]
pub struct BucketCfg {
    /// Set while a default retention or KMS/blocking default encryption exists, so the PUT path
    /// skips the lock when there is nothing to apply.
    defaults: AtomicBool,
    inner: RwLock<CfgInner>,
}

impl BucketCfg {
    fn refresh(&self, i: &CfgInner) {
        let kms = i
            .encryption
            .as_ref()
            .is_some_and(|e| e.algorithm.as_str() != ServerSideEncryption::AES256 || e.blocked.is_some());
        self.defaults.store(i.retention.is_some() || kms, Ordering::Release);
    }

    pub fn lock_enabled(&self) -> bool {
        self.inner.read().lock_enabled
    }

    pub fn enable_lock(&self) {
        self.inner.write().lock_enabled = true;
    }

    pub fn mfa_delete(&self) -> Option<bool> {
        self.inner.read().mfa_delete
    }

    pub fn set_mfa_delete(&self, on: bool) {
        self.inner.write().mfa_delete = Some(on);
    }

    pub fn encryption(&self) -> Option<EncryptionRule> {
        self.inner.read().encryption.clone()
    }

    pub fn set_encryption(&self, rule: Option<EncryptionRule>) {
        let mut i = self.inner.write();
        i.encryption = rule;
        self.refresh(&i);
    }

    pub fn set_default_retention(&self, d: Option<(LockMode, i32, bool)>) {
        let mut i = self.inner.write();
        i.retention = d.map(|(mode, n, years)| DefaultRetention {
            mode,
            period: if years { Period::Years(n) } else { Period::Days(n) },
        });
        self.refresh(&i);
    }

    /// `(mode, days or years, is_years)` of the default retention.
    pub fn default_retention(&self) -> Option<(LockMode, i32, bool)> {
        self.inner.read().retention.as_ref().map(|d| match d.period {
            Period::Days(n) => (d.mode, n, false),
            Period::Years(n) => (d.mode, n, true),
        })
    }

    fn has_defaults(&self) -> bool {
        self.defaults.load(Ordering::Acquire)
    }
}

/// What a request asked for, taken from its headers (PutObject, CopyObject, CreateMultipartUpload).
#[derive(Default)]
pub struct ObjectOpts {
    pub storage_class: Option<StorageClass>,
    pub website_redirect: Option<String>,
    pub sse: Option<ServerSideEncryption>,
    pub kms_key_id: Option<String>,
    pub kms_context: Option<String>,
    pub bucket_key: Option<bool>,
    pub sse_c_algorithm: Option<String>,
    pub sse_c_key: Option<String>,
    pub sse_c_key_md5: Option<String>,
    pub lock_mode: Option<ObjectLockMode>,
    pub lock_until: Option<Timestamp>,
    pub legal_hold: Option<ObjectLockLegalHoldStatus>,
}

/// Moves the object-attribute headers out of a PutObject / CopyObject / CreateMultipartUpload input.
macro_rules! object_opts {
    ($i:expr) => {
        $crate::objext::ObjectOpts {
            storage_class: $i.storage_class,
            website_redirect: $i.website_redirect_location,
            sse: $i.server_side_encryption,
            kms_key_id: $i.ssekms_key_id,
            kms_context: $i.ssekms_encryption_context,
            bucket_key: $i.bucket_key_enabled,
            sse_c_algorithm: $i.sse_customer_algorithm,
            sse_c_key: $i.sse_customer_key,
            sse_c_key_md5: $i.sse_customer_key_md5,
            lock_mode: $i.object_lock_mode,
            lock_until: $i.object_lock_retain_until_date,
            legal_hold: $i.object_lock_legal_hold_status,
        }
    };
}
pub(crate) use object_opts;

impl ObjectOpts {
    pub fn is_empty(&self) -> bool {
        self.storage_class.is_none()
            && self.website_redirect.is_none()
            && self.sse.is_none()
            && self.kms_key_id.is_none()
            && self.kms_context.is_none()
            && self.bucket_key.is_none()
            && self.sse_c_algorithm.is_none()
            && self.sse_c_key.is_none()
            && self.sse_c_key_md5.is_none()
            && self.lock_mode.is_none()
            && self.lock_until.is_none()
            && self.legal_hold.is_none()
    }
}

const STORAGE_CLASSES: &[&str] = &[
    StorageClass::STANDARD,
    StorageClass::REDUCED_REDUNDANCY,
    StorageClass::STANDARD_IA,
    StorageClass::ONEZONE_IA,
    StorageClass::INTELLIGENT_TIERING,
    StorageClass::GLACIER,
    StorageClass::DEEP_ARCHIVE,
    StorageClass::GLACIER_IR,
];

/// A version id as this server issues them (`null` or 32 hex digits); S3 rejects other strings
/// with 400 instead of looking them up.
pub fn check_version_id(id: &str) -> S3Result<()> {
    if id == "null" || (id.len() == 32 && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))) {
        Ok(())
    } else {
        Err(s3_error!(InvalidArgument, "Invalid version id specified"))
    }
}

/// Validates SSE-C headers and returns the canonical base64 MD5 of the key.
pub fn parse_sse_c(alg: Option<&str>, key: Option<&str>, md5: Option<&str>) -> S3Result<Option<String>> {
    if alg.is_none() && key.is_none() && md5.is_none() {
        return Ok(None);
    }
    let alg = alg.ok_or_else(|| {
        s3_error!(
            InvalidArgument,
            "Requests specifying Server Side Encryption with Customer provided keys must provide a valid encryption algorithm."
        )
    })?;
    if alg != "AES256" {
        return Err(s3_error!(
            InvalidEncryptionAlgorithmError,
            "The encryption request you specified is not valid. The valid value is AES256."
        ));
    }
    let key = key.ok_or_else(|| {
        s3_error!(
            InvalidArgument,
            "Requests specifying Server Side Encryption with Customer provided keys must provide an appropriate secret key."
        )
    })?;
    let md5 = md5.ok_or_else(|| {
        s3_error!(
            InvalidArgument,
            "Requests specifying Server Side Encryption with Customer provided keys must provide the client calculated MD5 of the secret key."
        )
    })?;
    let raw = base64_simd::STANDARD
        .decode_to_vec(key)
        .ok()
        .filter(|k| k.len() == 32)
        .ok_or_else(|| s3_error!(InvalidArgument, "The secret key was invalid for the specified algorithm."))?;
    let want = base64_simd::STANDARD.encode_to_string(crate::digest::md5([&raw]));
    if want != md5 {
        return Err(s3_error!(
            InvalidArgument,
            "The calculated MD5 hash of the key did not match the hash that was provided."
        ));
    }
    Ok(Some(want))
}

/// Checks the SSE-C headers of a read (or of a CopyObject source) against what the object was
/// stored with.
pub fn check_read_key(ext: Option<&ObjectExt>, alg: Option<&str>, key: Option<&str>, md5: Option<&str>) -> S3Result<()> {
    let stored = ext.and_then(ObjectExt::customer_key_md5);
    if stored.is_none() && alg.is_none() && key.is_none() && md5.is_none() {
        return Ok(());
    }
    let given = parse_sse_c(alg, key, md5)?;
    match (stored, given) {
        (Some(s), Some(g)) if s == g => Ok(()),
        (Some(_), Some(_)) => Err(s3_error!(AccessDenied, "Forbidden")),
        (Some(_), None) => Err(s3_error!(
            InvalidRequest,
            "The object was stored using a form of Server Side Encryption. The correct parameters must be provided to retrieve the object."
        )),
        (None, Some(_)) => Err(s3_error!(
            InvalidRequest,
            "The encryption parameters are not applicable to this object."
        )),
        (None, None) => Ok(()),
    }
}

fn validate_sse_algorithm(a: &ServerSideEncryption) -> S3Result<()> {
    match a.as_str() {
        ServerSideEncryption::AES256 | ServerSideEncryption::AWS_KMS | ServerSideEncryption::AWS_KMS_DSSE => Ok(()),
        _ => Err(s3_error!(InvalidArgument, "The encryption method specified is not supported")),
    }
}

/// Turns the attributes a write asked for into the object's [`ObjectExt`], applying the bucket's
/// default encryption. `None` means a plain object. The default retention is applied separately
/// ([`finish`]) because a multipart upload only knows its creation time on completion.
pub fn resolve(o: ObjectOpts, bucket: &Bucket, clock: &Clock) -> S3Result<Option<ObjectExt>> {
    if o.is_empty() && !bucket.cfg.has_defaults() {
        return Ok(None);
    }
    let mut ext = ObjectExt::default();

    if let Some(c) = o.storage_class {
        if !STORAGE_CLASSES.contains(&c.as_str()) {
            return Err(s3_error!(InvalidStorageClass, "The storage class you specified is not valid"));
        }
        if c.as_str() != StorageClass::STANDARD {
            ext.storage_class = Some(c);
        }
    }
    if let Some(r) = o.website_redirect {
        if !(r.starts_with('/') || r.starts_with("http://") || r.starts_with("https://")) {
            return Err(s3_error!(
                InvalidArgument,
                "The website redirect location must have a prefix of 'http://' or 'https://' or '/'."
            ));
        }
        ext.website_redirect = Some(r);
    }

    let customer = parse_sse_c(o.sse_c_algorithm.as_deref(), o.sse_c_key.as_deref(), o.sse_c_key_md5.as_deref())?;
    let rule = bucket.cfg.encryption();
    if let Some(key_md5) = customer {
        if o.sse.is_some() || o.kms_key_id.is_some() {
            return Err(s3_error!(
                InvalidArgument,
                "Server Side Encryption with Customer provided key is incompatible with the encryption method specified"
            ));
        }
        if rule.as_ref().is_some_and(|r| {
            r.blocked
                .as_ref()
                .is_some_and(|b| b.encryption_type.iter().flatten().any(|t| t.as_str() == EncryptionType::SSE_C))
        }) {
            return Err(s3_error!(
                AccessDenied,
                "Server side encryption with customer provided keys (SSE-C) is blocked for this bucket"
            ));
        }
        ext.sse = Some(Sse::Customer { key_md5 });
    } else if let Some(a) = &o.sse {
        validate_sse_algorithm(a)?;
        if a.as_str() == ServerSideEncryption::AES256 {
            if o.kms_key_id.is_some() {
                return Err(s3_error!(
                    InvalidArgument,
                    "A KMS key id can only be specified together with x-amz-server-side-encryption: aws:kms"
                ));
            }
        } else {
            ext.sse = Some(Sse::Kms {
                dsse: a.as_str() == ServerSideEncryption::AWS_KMS_DSSE,
                key_id: o.kms_key_id,
                context: o.kms_context,
                bucket_key: o.bucket_key,
            });
        }
    } else if o.kms_key_id.is_some() || o.kms_context.is_some() {
        return Err(s3_error!(
            InvalidArgument,
            "KMS parameters can only be specified together with x-amz-server-side-encryption: aws:kms"
        ));
    } else if let Some(r) = rule
        && r.algorithm.as_str() != ServerSideEncryption::AES256
    {
        ext.sse = Some(Sse::Kms {
            dsse: r.algorithm.as_str() == ServerSideEncryption::AWS_KMS_DSSE,
            key_id: r.kms_key_id,
            context: None,
            bucket_key: r.bucket_key,
        });
    }

    if o.lock_mode.is_some() || o.lock_until.is_some() || o.legal_hold.is_some() {
        if !bucket.cfg.lock_enabled() {
            return Err(missing_lock_config());
        }
        match (o.lock_mode, o.lock_until) {
            (Some(m), Some(until)) => {
                let mode = LockMode::parse(m.as_str()).ok_or_else(|| s3_error!(InvalidArgument, "Unknown wormMode directive"))?;
                if until <= Timestamp::from(clock.now()) {
                    return Err(s3_error!(InvalidArgument, "The retain until date must be in the future!"));
                }
                ext.retention = Some(Retention { mode, until });
            }
            (None, None) => {}
            _ => {
                return Err(s3_error!(
                    InvalidArgument,
                    "x-amz-object-lock-retain-until-date and x-amz-object-lock-mode must both be supplied"
                ));
            }
        }
        if let Some(h) = o.legal_hold {
            ext.legal_hold = Some(match h.as_str() {
                ObjectLockLegalHoldStatus::ON => true,
                ObjectLockLegalHoldStatus::OFF => false,
                _ => return Err(s3_error!(InvalidArgument, "Legal Hold must be either ON or OFF")),
            });
        }
    }
    // Explicit SSE-S3 headers and STANDARD leave nothing worth storing.
    Ok((!ext.is_plain()).then_some(ext))
}

/// Applies the bucket's default retention to an object without an explicit one.
pub fn finish(ext: Option<ObjectExt>, bucket: &Bucket, now: SystemTime) -> Option<Arc<ObjectExt>> {
    let mut ext = ext;
    if bucket.cfg.has_defaults()
        && let Some((mode, n, years)) = bucket.cfg.default_retention()
        && ext.as_ref().is_none_or(|e| e.retention.is_none())
    {
        let until = if years {
            add_years(now, n)
        } else {
            now + Duration::from_secs(n as u64 * 86_400)
        };
        ext.get_or_insert_default().retention = Some(Retention {
            mode,
            until: Timestamp::from(until),
        });
    }
    ext.map(Arc::new)
}

/// The encryption headers a response carries.
#[derive(Default)]
pub struct SseOut {
    pub algorithm: Option<ServerSideEncryption>,
    pub kms_key_id: Option<String>,
    pub bucket_key: Option<bool>,
    pub customer_algorithm: Option<String>,
    pub customer_key_md5: Option<String>,
}

pub fn sse_out(ext: Option<&ObjectExt>) -> SseOut {
    match ext.and_then(|e| e.sse.as_ref()) {
        None => SseOut {
            algorithm: Some(ServerSideEncryption::from_static(ServerSideEncryption::AES256)),
            ..Default::default()
        },
        Some(Sse::Kms {
            dsse, key_id, bucket_key, ..
        }) => SseOut {
            algorithm: Some(ServerSideEncryption::from_static(if *dsse {
                ServerSideEncryption::AWS_KMS_DSSE
            } else {
                ServerSideEncryption::AWS_KMS
            })),
            kms_key_id: Some(kms_arn(key_id.as_deref())),
            bucket_key: *bucket_key,
            ..Default::default()
        },
        Some(Sse::Customer { key_md5 }) => SseOut {
            customer_algorithm: Some("AES256".into()),
            customer_key_md5: Some(key_md5.clone()),
            ..Default::default()
        },
    }
}

pub fn sse_context(ext: Option<&ObjectExt>) -> Option<String> {
    match ext?.sse.as_ref()? {
        Sse::Kms { context, .. } => context.clone(),
        Sse::Customer { .. } => None,
    }
}

/// S3 reports the KMS key as an ARN whatever the client sent.
fn kms_arn(id: Option<&str>) -> String {
    match id {
        Some(id) if id.starts_with("arn:") => id.to_owned(),
        Some(id) if id.starts_with("alias/") => format!("arn:aws:kms:us-east-1:000000000000:{id}"),
        Some(id) => format!("arn:aws:kms:us-east-1:000000000000:key/{id}"),
        None => "arn:aws:kms:us-east-1:000000000000:key/00000000-0000-0000-0000-000000000000".to_owned(),
    }
}

/// Object Lock state as GET/HEAD report it.
pub fn lock_out(ext: &ObjectExt) -> (Option<ObjectLockMode>, Option<Timestamp>, Option<ObjectLockLegalHoldStatus>) {
    let hold = ext.legal_hold.map(|on| {
        ObjectLockLegalHoldStatus::from_static(if on {
            ObjectLockLegalHoldStatus::ON
        } else {
            ObjectLockLegalHoldStatus::OFF
        })
    });
    match &ext.retention {
        Some(r) => (Some(ObjectLockMode::from_static(r.mode.as_str())), Some(r.until.clone()), hold),
        None => (None, None, hold),
    }
}

/// Fills the extension-backed headers of a GetObject / HeadObject response.
macro_rules! apply_ext {
    ($out:expr, $ext:expr) => {{
        let ext: Option<&$crate::objext::ObjectExt> = $ext;
        $crate::objext::apply_sse!($out, $crate::objext::sse_out(ext));
        if let Some(e) = ext {
            $out.storage_class = e.storage_class.clone();
            $out.website_redirect_location = e.website_redirect.clone();
            (
                $out.object_lock_mode,
                $out.object_lock_retain_until_date,
                $out.object_lock_legal_hold_status,
            ) = $crate::objext::lock_out(e);
        }
    }};
}
pub(crate) use apply_ext;

/// Copies an [`SseOut`] into an output struct's encryption fields.
macro_rules! apply_sse {
    ($out:expr, $s:expr) => {{
        let s: $crate::objext::SseOut = $s;
        $out.server_side_encryption = s.algorithm;
        $out.ssekms_key_id = s.kms_key_id;
        $out.bucket_key_enabled = s.bucket_key;
        $out.sse_customer_algorithm = s.customer_algorithm;
        $out.sse_customer_key_md5 = s.customer_key_md5;
    }};
}
pub(crate) use apply_sse;

/// The storage class a response reports; S3 leaves the header out for STANDARD.
pub fn storage_class(ext: Option<&ObjectExt>) -> Option<StorageClass> {
    ext?.storage_class.clone()
}

/// Adds `years` calendar years, keeping month and day (Feb 29 becomes Feb 28).
pub fn add_years(t: SystemTime, years: i32) -> SystemTime {
    let d = t.duration_since(UNIX_EPOCH).unwrap_or_default();
    let days = (d.as_secs() / 86_400) as i64;
    let rest = d - Duration::from_secs(days as u64 * 86_400);
    let (y, m, day) = civil_from_days(days);
    let y = y + i64::from(years);
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let dim = match m {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    UNIX_EPOCH + Duration::from_secs(days_from_civil(y, m, day.min(dim)) as u64 * 86_400) + rest
}

// Howard Hinnant's civil-date algorithms.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (i64::from(m) + if m > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// The ETag S3 gives SSE-KMS / SSE-C objects: 32 hex digits that are not the content's MD5.
pub fn opaque_etag(md5: &[u8; 16]) -> String {
    let mut seed = md5.to_vec();
    seed.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    hex::encode(crate::digest::md5([&seed]))
}

/// The ETag of a copy: the source's, unless the two sides disagree on whether it is the content's MD5.
pub fn copy_etag(src: &Object, dst: Option<&ObjectExt>) -> String {
    let src_opaque = src.ext.as_ref().is_some_and(|e| e.sse.is_some());
    let dst_opaque = dst.is_some_and(|e| e.sse.is_some());
    if src_opaque == dst_opaque || src.parts_count.is_some() {
        return src.e_tag.clone();
    }
    if dst_opaque {
        let mut md5 = [0u8; 16];
        return match hex::decode_to_slice(&src.e_tag, &mut md5) {
            Ok(()) => opaque_etag(&md5),
            Err(_) => src.e_tag.clone(),
        };
    }
    hex::encode(crate::digest::md5(src.data.segments.iter()))
}

/// The ETag a write stores for content with this MD5. SSE-KMS and SSE-C objects don't get the MD5.
pub fn etag_for(md5: &[u8; 16], ext: Option<&ObjectExt>) -> String {
    match ext.and_then(|e| e.sse.as_ref()) {
        Some(_) => opaque_etag(md5),
        None => hex::encode(md5),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(s: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(s)
    }

    #[test]
    fn calendar_years() {
        // 2024-02-29T12:00:00Z + 1 year = 2025-02-28T12:00:00Z; + 4 years keeps Feb 29.
        let t = secs(1_709_208_000);
        assert_eq!(add_years(t, 1), secs(1_740_744_000));
        assert_eq!(add_years(t, 4), secs(1_835_438_400));
        assert_eq!(add_years(secs(0), 1), secs(365 * 86_400));
    }

    #[test]
    fn version_ids() {
        assert!(check_version_id("null").is_ok());
        assert!(check_version_id(&crate::store::new_version_id()).is_ok());
        assert!(check_version_id("nope").is_err());
        assert!(check_version_id("ABCDEF0123456789abcdef0123456789").is_err());
    }

    #[test]
    fn sse_c_validation() {
        let key = [7u8; 32];
        let key_b64 = base64_simd::STANDARD.encode_to_string(key);
        let md5 = base64_simd::STANDARD.encode_to_string(crate::digest::md5([&key]));
        assert_eq!(parse_sse_c(Some("AES256"), Some(&key_b64), Some(&md5)).unwrap(), Some(md5.clone()));
        assert_eq!(parse_sse_c(None, None, None).unwrap(), None);
        assert!(parse_sse_c(Some("AES128"), Some(&key_b64), Some(&md5)).is_err());
        assert!(parse_sse_c(Some("AES256"), Some(&key_b64), Some("AAAAAAAAAAAAAAAAAAAAAA==")).is_err());
        assert!(parse_sse_c(Some("AES256"), Some("c2hvcnQ="), Some(&md5)).is_err());
        assert!(parse_sse_c(Some("AES256"), Some(&key_b64), None).is_err());
    }

    #[test]
    fn retention_changes() {
        let now = Timestamp::from(secs(1_000));
        let r = |mode, s| Retention {
            mode,
            until: Timestamp::from(secs(s)),
        };
        let gov = r(LockMode::Governance, 2_000);
        let comp = r(LockMode::Compliance, 2_000);
        // Extending is always fine, shortening needs bypass (governance) or is refused (compliance).
        assert!(check_retention_change(Some(&gov), Some(&r(LockMode::Governance, 3_000)), &now, false).is_ok());
        assert!(check_retention_change(Some(&gov), Some(&r(LockMode::Governance, 1_500)), &now, false).is_err());
        assert!(check_retention_change(Some(&gov), None, &now, true).is_ok());
        assert!(check_retention_change(Some(&gov), Some(&r(LockMode::Compliance, 2_000)), &now, false).is_ok());
        assert!(check_retention_change(Some(&comp), Some(&r(LockMode::Governance, 3_000)), &now, true).is_err());
        assert!(check_retention_change(Some(&comp), None, &now, true).is_err());
        // Expired retention no longer constrains anything.
        let later = Timestamp::from(secs(5_000));
        assert!(check_retention_change(Some(&comp), None, &later, false).is_ok());
    }
}
