//! Authorization: bucket policies and ACLs applied to requests.
//!
//! s3s calls [`S3Access::check`] for every request when authentication is configured, signed or
//! not. The model is the one S3 uses inside a single account:
//!
//! * A signed request is the account root. It is allowed unless a bucket policy explicitly denies
//!   it (the policy-management operations are exempt, so a Deny-everyone policy cannot lock the
//!   owner out of fixing it).
//! * An anonymous request is allowed only if an Allow statement for everyone in the bucket policy
//!   or a public ACL grant says so, and no statement denies it. Block Public Access settings
//!   (`IgnorePublicAcls`, `RestrictPublicBuckets`) are honored.
//!
//! Until some bucket has had a policy attached, a signed request returns from `check` after one
//! atomic load, so deployments that never use policies pay nothing. Anonymous requests are rare
//! and always take the full path. Without an auth provider (`--no-auth`) s3s never calls `check`, so
//! nothing is authorized or denied: signed and anonymous requests alike act as the owner.

use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use s3s::access::{S3Access, S3AccessContext};
use s3s::dto::{CopySource, DeleteObjectsInput, PostObjectInput};
use s3s::path::S3Path;
use s3s::{S3Error, S3ErrorCode, S3Request, S3Result};

use crate::acl::Perm;
use crate::clock::Clock;
use crate::policy::{Context, Verdict};
use crate::store::{Bucket, Store};

/// The client address, recorded by the server only while some policy has an `aws:SourceIp` condition.
#[derive(Clone, Copy)]
pub struct PeerAddr(pub IpAddr);

/// Keys of a DeleteObjects request the policy denies, for the operation to report as errors.
#[derive(Clone)]
pub struct Denied(pub Vec<(String, Option<String>)>);

pub struct Access {
    pub store: Arc<Store>,
    pub clock: Arc<Clock>,
    /// Whether signatures are verified. Without it s3s never calls `check` and every request is
    /// accepted, so the per-operation hooks must not mistake unsigned requests for anonymous ones.
    pub auth: bool,
}

pub fn denied() -> S3Error {
    S3Error::with_message(S3ErrorCode::AccessDenied, "Access Denied")
}

/// The IAM action a request needs, as bucket policies spell it.
fn action_of(op: &str, version: bool) -> String {
    let v = |plain: &str, versioned: &str| format!("s3:{}", if version { versioned } else { plain });
    match op {
        "ListObjects" | "ListObjectsV2" | "HeadBucket" => "s3:ListBucket".into(),
        "ListObjectVersions" => "s3:ListBucketVersions".into(),
        "ListMultipartUploads" => "s3:ListBucketMultipartUploads".into(),
        "ListParts" => "s3:ListMultipartUploadParts".into(),
        "ListBuckets" => "s3:ListAllMyBuckets".into(),
        "GetObject" | "HeadObject" | "GetObjectAttributes" => v("GetObject", "GetObjectVersion"),
        "GetObjectLockConfiguration" => "s3:GetBucketObjectLockConfiguration".into(),
        "PutObjectLockConfiguration" => "s3:PutBucketObjectLockConfiguration".into(),
        "GetBucketEncryption" => "s3:GetEncryptionConfiguration".into(),
        "PutBucketEncryption" | "DeleteBucketEncryption" => "s3:PutEncryptionConfiguration".into(),
        "DeleteObject" | "DeleteObjects" => v("DeleteObject", "DeleteObjectVersion"),
        "GetObjectTagging" => v("GetObjectTagging", "GetObjectVersionTagging"),
        "PutObjectTagging" => v("PutObjectTagging", "PutObjectVersionTagging"),
        "DeleteObjectTagging" => v("DeleteObjectTagging", "DeleteObjectVersionTagging"),
        "GetObjectAcl" => v("GetObjectAcl", "GetObjectVersionAcl"),
        "PutObjectAcl" => v("PutObjectAcl", "PutObjectVersionAcl"),
        "CreateMultipartUpload" | "UploadPart" | "UploadPartCopy" | "CompleteMultipartUpload" | "CopyObject" | "PostObject" => {
            "s3:PutObject".into()
        }
        "GetBucketLifecycleConfiguration" => "s3:GetLifecycleConfiguration".into(),
        "PutBucketLifecycleConfiguration" | "DeleteBucketLifecycle" => "s3:PutLifecycleConfiguration".into(),
        "GetBucketIntelligentTieringConfiguration" | "ListBucketIntelligentTieringConfigurations" => {
            "s3:GetIntelligentTieringConfiguration".into()
        }
        "PutBucketIntelligentTieringConfiguration" | "DeleteBucketIntelligentTieringConfiguration" => {
            "s3:PutIntelligentTieringConfiguration".into()
        }
        "GetBucketCors" => "s3:GetBucketCORS".into(),
        "PutBucketCors" | "DeleteBucketCors" => "s3:PutBucketCORS".into(),
        "GetPublicAccessBlock" => "s3:GetBucketPublicAccessBlock".into(),
        "PutPublicAccessBlock" | "DeletePublicAccessBlock" => "s3:PutBucketPublicAccessBlock".into(),
        "GetBucketNotificationConfiguration" => "s3:GetBucketNotification".into(),
        "PutBucketNotificationConfiguration" => "s3:PutBucketNotification".into(),
        "DeleteBucketOwnershipControls" => "s3:PutBucketOwnershipControls".into(),
        "DeleteBucketTagging" => "s3:PutBucketTagging".into(),
        other => format!("s3:{other}"),
    }
}

fn arn(bucket: &str, key: Option<&str>) -> String {
    match key {
        Some(k) => format!("arn:aws:s3:::{bucket}/{k}"),
        None => format!("arn:aws:s3:::{bucket}"),
    }
}

enum Scope {
    Bucket,
    Object,
}

/// The ACL permission that lets an anonymous caller perform `op`.
fn acl_need(op: &str) -> Option<(Scope, Perm)> {
    Some(match op {
        "GetObject" | "HeadObject" | "GetObjectAttributes" => (Scope::Object, Perm::Read),
        "GetObjectAcl" => (Scope::Object, Perm::ReadAcp),
        "PutObjectAcl" => (Scope::Object, Perm::WriteAcp),
        "ListObjects" | "ListObjectsV2" | "ListObjectVersions" | "ListMultipartUploads" | "HeadBucket" => (Scope::Bucket, Perm::Read),
        "PutObject"
        | "DeleteObject"
        | "DeleteObjects"
        | "CreateMultipartUpload"
        | "UploadPart"
        | "UploadPartCopy"
        | "CompleteMultipartUpload"
        | "AbortMultipartUpload"
        | "CopyObject"
        | "PostObject" => (Scope::Bucket, Perm::Write),
        "GetBucketAcl" => (Scope::Bucket, Perm::ReadAcp),
        "PutBucketAcl" => (Scope::Bucket, Perm::WriteAcp),
        _ => return None,
    })
}

/// Operations the account root may always perform, even under a Deny-everything policy.
fn is_policy_admin(op: &str) -> bool {
    matches!(op, "GetBucketPolicy" | "PutBucketPolicy" | "DeleteBucketPolicy")
}

fn query_param(query: &str, name: &str) -> Option<String> {
    if !query.contains(name) {
        return None;
    }
    form_urlencoded::parse(query.as_bytes())
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.into_owned())
}

impl Access {
    /// Whether the request may perform `op` on `key` (`None` for bucket-level operations).
    fn permit(&self, bucket: &Bucket, name: &str, op: &str, key: Option<&str>, version_id: Option<&str>, ctx: &Context<'_>) -> bool {
        let policy = bucket.config.policy.read().clone();
        let pab = bucket.config.public_access_block.read().unwrap_or_default();
        let mut allowed_by_policy = false;
        if let Some(p) = &policy {
            match p.evaluate(&action_of(op, version_id.is_some()), &arn(name, key), ctx) {
                Verdict::Deny if !(ctx.signed && is_policy_admin(op)) => return false,
                Verdict::Allow => allowed_by_policy = !(pab.restrict_public_buckets && p.public),
                _ => {}
            }
        }
        if ctx.signed || allowed_by_policy {
            return true;
        }
        if !bucket.config.acls_enabled() || pab.ignore_public_acls {
            return false;
        }
        let Some((scope, perm)) = acl_need(op) else { return false };
        let bucket_allows = |perm| bucket.config.acl.read().allows_anonymous(perm);
        match scope {
            Scope::Bucket => bucket_allows(perm),
            Scope::Object => {
                let objects = bucket.objects.read();
                let version = key.and_then(|k| objects.get(k)).and_then(|e| match version_id {
                    Some(id) => e.find(id),
                    None => e.latest(),
                });
                match version.and_then(|v| v.obj.as_ref()) {
                    Some(o) => o.acl.as_ref().is_some_and(|a| a.allows_anonymous(perm)),
                    // Nothing to read: listing permission decides between 404 and 403.
                    None => perm == Perm::Read && bucket_allows(Perm::Read),
                }
            }
        }
    }

    fn peer(&self, ext: &mut http::Extensions) -> Option<IpAddr> {
        ext.get::<PeerAddr>().map(|p| p.0)
    }

    fn missing_bucket(signed: bool, op: &str) -> S3Result<()> {
        if signed {
            Ok(())
        } else if op == "CreateBucket" {
            Err(denied())
        } else {
            Err(S3Error::new(S3ErrorCode::NoSuchBucket))
        }
    }
}

#[async_trait::async_trait]
impl S3Access for Access {
    async fn check(&self, cx: &mut S3AccessContext<'_>) -> S3Result<()> {
        let signed = cx.credentials().is_some();
        if signed && !self.store.access.any_policy.load(Ordering::Relaxed) {
            return Ok(());
        }
        let peer = self.peer(cx.extensions_mut());
        let (name, key) = match cx.s3_path() {
            S3Path::Root => return if signed { Ok(()) } else { Err(denied()) },
            S3Path::Bucket { bucket } => (bucket.to_string(), None),
            S3Path::Object { bucket, key } => (bucket.to_string(), Some(key.to_string())),
        };
        let op = cx.s3_op().name();
        // Their keys are in the body or form; `delete_objects` / `post_object` below decide.
        if matches!(op, "DeleteObjects" | "PostObject") {
            return Ok(());
        }
        let Some(bucket) = self.store.bucket(&name) else {
            return Self::missing_bucket(signed, op);
        };
        let query = cx.uri().query().unwrap_or("");
        let version_id = key.as_ref().and_then(|_| query_param(query, "versionId"));
        let ctx = Context {
            headers: cx.headers(),
            query,
            now: self.clock.now(),
            signed,
            peer,
        };
        if !self.permit(&bucket, &name, op, key.as_deref(), version_id.as_deref(), &ctx) {
            return Err(denied());
        }
        if matches!(op, "CopyObject" | "UploadPartCopy")
            && let Some(src) = ctx.headers.get("x-amz-copy-source").and_then(|v| v.to_str().ok())
            && let Ok(CopySource::Bucket {
                bucket: src_bucket,
                key: src_key,
                version_id,
            }) = CopySource::parse(src)
            && let Some(sb) = self.store.bucket(&src_bucket)
            && !self.permit(&sb, &src_bucket, "GetObject", Some(&src_key), version_id.as_deref(), &ctx)
        {
            return Err(denied());
        }
        Ok(())
    }

    async fn delete_objects(&self, req: &mut S3Request<DeleteObjectsInput>) -> S3Result<()> {
        let signed = req.credentials.is_some();
        if !self.auth || signed && !self.store.access.any_policy.load(Ordering::Relaxed) {
            return Ok(());
        }
        let Some(bucket) = self.store.bucket(&req.input.bucket) else {
            return Self::missing_bucket(signed, "DeleteObjects");
        };
        let peer = self.peer(&mut req.extensions);
        let ctx = Context {
            headers: &req.headers,
            query: req.uri.query().unwrap_or(""),
            now: self.clock.now(),
            signed,
            peer,
        };
        let denied: Vec<_> = req
            .input
            .delete
            .objects
            .iter()
            .filter(|o| {
                !self.permit(
                    &bucket,
                    &req.input.bucket,
                    "DeleteObjects",
                    Some(&o.key),
                    o.version_id.as_deref(),
                    &ctx,
                )
            })
            .map(|o| (o.key.clone(), o.version_id.clone()))
            .collect();
        if !denied.is_empty() {
            req.extensions.insert(Denied(denied));
        }
        Ok(())
    }

    async fn post_object(&self, req: &mut S3Request<PostObjectInput>) -> S3Result<()> {
        let signed = req.credentials.is_some();
        if !self.auth || signed && !self.store.access.any_policy.load(Ordering::Relaxed) {
            return Ok(());
        }
        let Some(bucket) = self.store.bucket(&req.input.bucket) else {
            return Self::missing_bucket(signed, "PostObject");
        };
        let peer = self.peer(&mut req.extensions);
        let ctx = Context {
            headers: &req.headers,
            query: "",
            now: self.clock.now(),
            signed,
            peer,
        };
        if self.permit(&bucket, &req.input.bucket, "PostObject", Some(&req.input.key), None, &ctx) {
            Ok(())
        } else {
            Err(denied())
        }
    }
}
