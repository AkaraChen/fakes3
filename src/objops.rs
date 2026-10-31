//! Handlers for the object-level operations that exist only for Object Lock, encryption and
//! GetObjectAttributes. `s3impl` forwards the matching `S3` trait methods here.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;

use s3s::dto::*;
use s3s::{S3Error, S3ErrorCode, S3Request, S3Response, S3Result, s3_error};

use crate::objext::{self, BucketCfg, EncryptionRule, LockMode, Retention};
use crate::s3impl::{FakeS3, obj, ts};
use crate::store::{Bucket as StoreBucket, Entry, Object as StoreObject, Version, Versioning};

impl FakeS3 {
    /// Replaces one object version with an edited copy (versions are shared immutably).
    pub(crate) fn modify_object(
        &self,
        b: &StoreBucket,
        key: &str,
        version_id: Option<&str>,
        f: impl FnOnce(&mut StoreObject) -> S3Result<()>,
    ) -> S3Result<()> {
        if let Some(id) = version_id {
            objext::check_version_id(id)?;
        }
        let mut objects = b.objects.write();
        let entry = objects.get_mut(key);
        let pos = match (version_id, &entry) {
            (None, Some(e)) => e.versions.len().checked_sub(1),
            (Some(id), Some(e)) => e.versions.iter().rposition(|v| v.id == id),
            _ => None,
        };
        let (Some(entry), Some(pos)) = (entry, pos) else {
            return Err(if version_id.is_some() {
                s3_error!(NoSuchVersion)
            } else {
                s3_error!(NoSuchKey)
            });
        };
        let v = &entry.versions[pos];
        let Some(o) = &v.obj else {
            return Err(if version_id.is_some() {
                s3_error!(MethodNotAllowed)
            } else {
                s3_error!(NoSuchKey)
            });
        };
        let mut o = o.clone();
        f(&mut o)?;
        entry.versions[pos] = Arc::new(Version {
            id: v.id.clone(),
            obj: Some(o),
            last_modified: v.last_modified,
        });
        Ok(())
    }

    fn lock_bucket(&self, name: &str) -> S3Result<Arc<StoreBucket>> {
        let b = self.bucket(name)?;
        if !b.cfg.lock_enabled() {
            return Err(objext::missing_lock_config());
        }
        Ok(b)
    }

    pub(crate) async fn do_put_object_lock_configuration(
        &self,
        req: S3Request<PutObjectLockConfigurationInput>,
    ) -> S3Result<S3Response<PutObjectLockConfigurationOutput>> {
        let b = self.bucket(&req.input.bucket)?;
        let cfg = req.input.object_lock_configuration.ok_or_else(|| s3_error!(MalformedXML))?;
        if cfg.object_lock_enabled.as_ref().map(|e| e.as_str()) != Some(ObjectLockEnabled::ENABLED) {
            return Err(s3_error!(MalformedXML));
        }
        let retention = match cfg.rule.and_then(|r| r.default_retention) {
            None => None,
            Some(d) => {
                let mode = d
                    .mode
                    .as_ref()
                    .and_then(|m| LockMode::parse(m.as_str()))
                    .ok_or_else(|| s3_error!(MalformedXML))?;
                let (n, years) = match (d.days, d.years) {
                    (Some(n), None) => (n, false),
                    (None, Some(n)) => (n, true),
                    _ => return Err(s3_error!(MalformedXML)),
                };
                if n <= 0 {
                    let mut e = S3Error::with_message(
                        S3ErrorCode::Custom("InvalidRetentionPeriod".into()),
                        "Default retention period must be a positive integer value",
                    );
                    e.set_status_code(http::StatusCode::BAD_REQUEST);
                    return Err(e);
                }
                Some((mode, n, years))
            }
        };
        if *b.versioning.read() != Versioning::Enabled {
            return Err(s3_error!(
                InvalidBucketState,
                "Versioning must be 'Enabled' on the bucket to apply a Object Lock configuration"
            ));
        }
        b.cfg.enable_lock();
        b.cfg.set_default_retention(retention);
        Ok(S3Response::new(PutObjectLockConfigurationOutput::default()))
    }

    pub(crate) async fn do_get_object_lock_configuration(
        &self,
        req: S3Request<GetObjectLockConfigurationInput>,
    ) -> S3Result<S3Response<GetObjectLockConfigurationOutput>> {
        let b = self.bucket(&req.input.bucket)?;
        if !b.cfg.lock_enabled() {
            return Err(s3_error!(
                ObjectLockConfigurationNotFoundError,
                "Object Lock configuration does not exist for this bucket"
            ));
        }
        let rule = b.cfg.default_retention().map(|(mode, n, years)| ObjectLockRule {
            default_retention: Some(DefaultRetention {
                mode: Some(ObjectLockRetentionMode::from_static(mode.as_str())),
                days: (!years).then_some(n),
                years: years.then_some(n),
            }),
        });
        Ok(S3Response::new(GetObjectLockConfigurationOutput {
            object_lock_configuration: Some(ObjectLockConfiguration {
                object_lock_enabled: Some(ObjectLockEnabled::from_static(ObjectLockEnabled::ENABLED)),
                rule,
            }),
        }))
    }

    pub(crate) async fn do_put_object_retention(
        &self,
        req: S3Request<PutObjectRetentionInput>,
    ) -> S3Result<S3Response<PutObjectRetentionOutput>> {
        let input = req.input;
        let b = self.lock_bucket(&input.bucket)?;
        // An empty <Retention/> clears the retention; mode and date only make sense together.
        let new = match input.retention.map(|r| (r.mode, r.retain_until_date)) {
            None | Some((Some(_), None)) | Some((None, Some(_))) => return Err(s3_error!(MalformedXML)),
            Some((None, None)) => None,
            Some((Some(m), Some(until))) => Some(Retention {
                mode: LockMode::parse(m.as_str()).ok_or_else(|| s3_error!(MalformedXML))?,
                until,
            }),
        };
        let now = ts(self.clock.now());
        if new.as_ref().is_some_and(|r| r.until <= now) {
            return Err(s3_error!(InvalidArgument, "The retain until date must be in the future!"));
        }
        let bypass = input.bypass_governance_retention.unwrap_or(false);
        self.modify_object(&b, &input.key, input.version_id.as_deref(), |o| {
            let old = o.ext.as_ref().and_then(|e| e.retention.as_ref());
            objext::check_retention_change(old, new.as_ref(), &now, bypass)?;
            let mut ext = o.ext.as_deref().cloned().unwrap_or_default();
            ext.retention = new;
            o.ext = Some(Arc::new(ext));
            Ok(())
        })?;
        Ok(S3Response::new(PutObjectRetentionOutput::default()))
    }

    pub(crate) async fn do_get_object_retention(
        &self,
        req: S3Request<GetObjectRetentionInput>,
    ) -> S3Result<S3Response<GetObjectRetentionOutput>> {
        let input = req.input;
        self.lock_bucket(&input.bucket)?;
        let (v, _) = self.version(&input.bucket, &input.key, input.version_id.as_deref())?;
        let r = obj(&v).ext.as_ref().and_then(|e| e.retention.clone()).ok_or_else(|| {
            s3_error!(
                NoSuchObjectLockConfiguration,
                "The specified object does not have a ObjectLock configuration"
            )
        })?;
        Ok(S3Response::new(GetObjectRetentionOutput {
            retention: Some(ObjectLockRetention {
                mode: Some(ObjectLockRetentionMode::from_static(r.mode.as_str())),
                retain_until_date: Some(r.until),
            }),
        }))
    }

    pub(crate) async fn do_put_object_legal_hold(
        &self,
        req: S3Request<PutObjectLegalHoldInput>,
    ) -> S3Result<S3Response<PutObjectLegalHoldOutput>> {
        let input = req.input;
        let b = self.lock_bucket(&input.bucket)?;
        let on = match input.legal_hold.and_then(|h| h.status).as_ref().map(|s| s.as_str()) {
            Some(ObjectLockLegalHoldStatus::ON) => true,
            Some(ObjectLockLegalHoldStatus::OFF) => false,
            _ => return Err(s3_error!(MalformedXML)),
        };
        self.modify_object(&b, &input.key, input.version_id.as_deref(), |o| {
            let mut ext = o.ext.as_deref().cloned().unwrap_or_default();
            ext.legal_hold = Some(on);
            o.ext = Some(Arc::new(ext));
            Ok(())
        })?;
        Ok(S3Response::new(PutObjectLegalHoldOutput::default()))
    }

    pub(crate) async fn do_get_object_legal_hold(
        &self,
        req: S3Request<GetObjectLegalHoldInput>,
    ) -> S3Result<S3Response<GetObjectLegalHoldOutput>> {
        let input = req.input;
        self.lock_bucket(&input.bucket)?;
        let (v, _) = self.version(&input.bucket, &input.key, input.version_id.as_deref())?;
        let on = obj(&v).ext.as_ref().and_then(|e| e.legal_hold).ok_or_else(|| {
            s3_error!(
                NoSuchObjectLockConfiguration,
                "The specified object does not have a ObjectLock configuration"
            )
        })?;
        let status = if on {
            ObjectLockLegalHoldStatus::ON
        } else {
            ObjectLockLegalHoldStatus::OFF
        };
        Ok(S3Response::new(GetObjectLegalHoldOutput {
            legal_hold: Some(ObjectLockLegalHold {
                status: Some(ObjectLockLegalHoldStatus::from_static(status)),
            }),
        }))
    }

    pub(crate) async fn do_put_bucket_encryption(
        &self,
        req: S3Request<PutBucketEncryptionInput>,
    ) -> S3Result<S3Response<PutBucketEncryptionOutput>> {
        let b = self.bucket(&req.input.bucket)?;
        let mut rules = req.input.server_side_encryption_configuration.rules;
        if rules.len() != 1 {
            return Err(s3_error!(MalformedXML));
        }
        let rule = rules.remove(0);
        // A rule that only sets BlockedEncryptionTypes (how SSE-C is blocked or unblocked) keeps SSE-S3.
        let by_default = rule
            .apply_server_side_encryption_by_default
            .unwrap_or_else(|| ServerSideEncryptionByDefault {
                sse_algorithm: ServerSideEncryption::from_static(ServerSideEncryption::AES256),
                kms_master_key_id: None,
            });
        let algorithm = by_default.sse_algorithm;
        let kms = match algorithm.as_str() {
            ServerSideEncryption::AES256 => false,
            ServerSideEncryption::AWS_KMS | ServerSideEncryption::AWS_KMS_DSSE => true,
            _ => return Err(s3_error!(MalformedXML)),
        };
        if !kms && by_default.kms_master_key_id.is_some() {
            return Err(s3_error!(InvalidArgument, "a KMS key id can only be given for aws:kms encryption"));
        }
        b.cfg.set_encryption(Some(EncryptionRule {
            algorithm,
            kms_key_id: by_default.kms_master_key_id,
            bucket_key: rule.bucket_key_enabled,
            blocked: rule.blocked_encryption_types,
        }));
        Ok(S3Response::new(PutBucketEncryptionOutput::default()))
    }

    pub(crate) async fn do_get_bucket_encryption(
        &self,
        req: S3Request<GetBucketEncryptionInput>,
    ) -> S3Result<S3Response<GetBucketEncryptionOutput>> {
        let b = self.bucket(&req.input.bucket)?;
        // Pre-2023 behaviour: a bucket without default encryption (`--bucket-defaults legacy`).
        let r = b.cfg.encryption().ok_or_else(|| {
            let mut e = s3_error!(
                ServerSideEncryptionConfigurationNotFoundError,
                "The server side encryption configuration was not found"
            );
            e.set_status_code(http::StatusCode::NOT_FOUND);
            e
        })?;
        let rule = ServerSideEncryptionRule {
            apply_server_side_encryption_by_default: Some(ServerSideEncryptionByDefault {
                sse_algorithm: r.algorithm,
                kms_master_key_id: r.kms_key_id,
            }),
            bucket_key_enabled: r.bucket_key,
            blocked_encryption_types: r.blocked,
        };
        Ok(S3Response::new(GetBucketEncryptionOutput {
            server_side_encryption_configuration: Some(ServerSideEncryptionConfiguration { rules: vec![rule] }),
        }))
    }

    pub(crate) async fn do_delete_bucket_encryption(
        &self,
        req: S3Request<DeleteBucketEncryptionInput>,
    ) -> S3Result<S3Response<DeleteBucketEncryptionOutput>> {
        // AWS no longer lets a bucket go without default encryption: deleting it restores SSE-S3.
        self.bucket(&req.input.bucket)?
            .cfg
            .set_encryption(self.defaults.default_encryption());
        Ok(S3Response::new(DeleteBucketEncryptionOutput {}))
    }

    pub(crate) async fn do_get_object_attributes(
        &self,
        req: S3Request<GetObjectAttributesInput>,
    ) -> S3Result<S3Response<GetObjectAttributesOutput>> {
        let input = req.input;
        let (v, versioned) = self.version(&input.bucket, &input.key, input.version_id.as_deref())?;
        let o = obj(&v);
        objext::check_read_key(
            o.ext.as_deref(),
            input.sse_customer_algorithm.as_deref(),
            input.sse_customer_key.as_deref(),
            input.sse_customer_key_md5.as_deref(),
        )?;
        if input.object_attributes.iter().all(|a| a.as_str().trim().is_empty()) {
            return Err(s3_error!(
                InvalidArgument,
                "The x-amz-object-attributes header specifying the attributes to be retrieved is either missing or empty"
            ));
        }
        let (mut etag, mut checksum, mut parts, mut size, mut class) = (false, false, false, false, false);
        // SDKs send one comma-joined header; s3s hands it over unsplit.
        for a in input.object_attributes.iter().flat_map(|a| a.as_str().split(',')).map(str::trim) {
            match a {
                ObjectAttributes::ETAG => etag = true,
                ObjectAttributes::CHECKSUM => checksum = true,
                ObjectAttributes::OBJECT_PARTS => parts = true,
                ObjectAttributes::OBJECT_SIZE => size = true,
                ObjectAttributes::STORAGE_CLASS => class = true,
                _ => return Err(s3_error!(InvalidArgument, "Invalid attribute name specified.")),
            }
        }
        let mut out = GetObjectAttributesOutput {
            last_modified: Some(ts(o.last_modified)),
            version_id: versioned.then(|| v.id.clone()),
            ..Default::default()
        };
        if etag {
            out.e_tag = Some(ETag::Strong(o.e_tag.clone()));
        }
        if size {
            out.object_size = Some(o.data.size as i64);
        }
        if class {
            let c = objext::storage_class(o.ext.as_deref()).unwrap_or_else(|| StorageClass::from_static(StorageClass::STANDARD));
            out.storage_class = Some(c);
        }
        if checksum && o.checksum != Checksum::default() {
            out.checksum = Some(Checksum {
                checksum_type: o.checksum_type.clone(),
                ..o.checksum.clone()
            });
        }
        if parts && let (Some(count), Some(sizes)) = (o.parts_count, &o.part_sizes) {
            let max = input.max_parts.unwrap_or(1000).clamp(0, 1000) as usize;
            let marker = input.part_number_marker.unwrap_or(0).max(0) as usize;
            let part_checksums = o.ext.as_ref().and_then(|e| e.part_checksums.as_ref());
            let mut list = Vec::new();
            for (i, size) in sizes.iter().enumerate().skip(marker) {
                if list.len() == max {
                    break;
                }
                let c = part_checksums.and_then(|c| c.get(i)).cloned().unwrap_or_default();
                list.push(ObjectPart {
                    part_number: Some(i as i32 + 1),
                    size: Some(*size as i64),
                    checksum_crc32: c.checksum_crc32,
                    checksum_crc32c: c.checksum_crc32c,
                    checksum_crc64nvme: c.checksum_crc64nvme,
                    checksum_sha1: c.checksum_sha1,
                    checksum_sha256: c.checksum_sha256,
                    ..Default::default()
                });
            }
            let next = marker + list.len();
            let truncated = next < count as usize;
            out.object_parts = Some(GetObjectAttributesParts {
                total_parts_count: Some(count),
                part_number_marker: Some(marker as i32),
                next_part_number_marker: truncated.then_some(next as i32),
                max_parts: Some(max as i32),
                is_truncated: Some(truncated),
                parts: Some(list),
            });
        }
        Ok(S3Response::new(out))
    }
}

/// Whether Object Lock forbids permanently deleting this version right now.
pub fn version_locked(
    b: &StoreBucket,
    objects: &BTreeMap<String, Entry>,
    key: &str,
    id: &str,
    now: SystemTime,
    bypass_governance: Option<bool>,
) -> bool {
    b.cfg.lock_enabled()
        && objects
            .get(key)
            .and_then(|e| e.find(id))
            .and_then(|v| v.obj.as_ref())
            .is_some_and(|o| objext::delete_blocked(o, now, bypass_governance.unwrap_or(false)))
}

/// Whether the bucket needs the MFA header for `DeleteObject`/`DeleteObjects` of versions.
pub fn mfa_required(cfg: &BucketCfg, mfa: Option<&str>) -> S3Result<()> {
    if cfg.mfa_delete() == Some(true) {
        match mfa {
            None => return Err(s3_error!(AccessDenied, "Mfa Authentication must be used for this request")),
            Some(m) => check_mfa_header(m)?,
        }
    }
    Ok(())
}

/// `x-amz-mfa` is `<device serial> <6 digit token>`. There is no device to ask, so any
/// well-formed value passes.
pub fn check_mfa_header(m: &str) -> S3Result<()> {
    match m.rsplit_once(' ') {
        Some((serial, token)) if !serial.trim().is_empty() && token.len() == 6 && token.bytes().all(|b| b.is_ascii_digit()) => Ok(()),
        _ => Err(s3_error!(
            InvalidRequest,
            "Invalid MFA header: expected '<serial number> <six digit token>'"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mfa_header_format() {
        assert!(check_mfa_header("arn:aws:iam::123456789012:mfa/user 123456").is_ok());
        assert!(check_mfa_header("GAHT12345678 000000").is_ok());
        assert!(check_mfa_header("123456").is_err());
        assert!(check_mfa_header("serial 12345").is_err());
        assert!(check_mfa_header("serial abcdef").is_err());
    }
}
