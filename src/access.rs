//! Authorization: bucket policies and ACLs applied to requests.
//!
//! s3s calls [`S3Access::check`] for every request when an auth provider is configured, signed or
//! not (fakes3 always configures one). Who a request acts as is an [`Identity`]: anonymous, or one
//! of the accounts of [`crate::users`]. The rules (`docs/multi-user.md` has the reasoning):
//!
//! * A bucket policy's explicit Deny always wins (the policy-management operations are exempt for
//!   the bucket owner, so a Deny-everyone policy cannot lock them out of fixing it).
//! * The bucket owner may do anything to its bucket and to the objects it owns. Objects another
//!   account owns (uploaded under `ObjectWriter` ownership) are theirs: the bucket owner needs an
//!   ACL grant to read them, and a policy Allow does not cover them.
//! * Any other signed account, and the anonymous caller, needs an Allow from the bucket policy or
//!   a grant in the ACL (to its canonical id, to `AuthenticatedUsers`, or for everyone to
//!   `AllUsers`). Block Public Access (`IgnorePublicAcls`, `RestrictPublicBuckets`) is honored.
//! * An object's owner can always use its own object (the default ACL) and read and write its ACL.
//!
//! What "signed" means depends on the mode, and [`identify`] is the only place that decides: with
//! `--auth` s3s verified a signature and hands over credentials; without it the signature was
//! stripped unverified before s3s saw the request (see [`crate::noauth`]) and left a
//! [`Signed`] extension behind. Either way the access key picks the identity.
//!
//! With a single identity and no bucket policy anywhere, a signed request returns from `check`
//! after one atomic load, so deployments that never use policies or several users pay nothing.
//! Anonymous requests are rare and always take the full path.

use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use s3s::access::{S3Access, S3AccessContext};
use s3s::dto::{CopySource, DeleteObjectsInput, PostObjectInput};
use s3s::path::S3Path;
use s3s::{S3Error, S3ErrorCode, S3Request, S3Result};

use crate::acl::Perm;
use crate::clock::Clock;
use crate::noauth::Signed;
use crate::policy::{Context, Verdict};
use crate::store::{Bucket, Store};
use crate::users::{UserId, Users};

/// The client address, recorded by the server only while some policy has an `aws:SourceIp` condition.
#[derive(Clone, Copy)]
pub struct PeerAddr(pub IpAddr);

/// Keys of a DeleteObjects request the policy denies, for the operation to report as errors.
#[derive(Clone)]
pub struct Denied(pub Vec<(String, Option<String>)>);

pub struct Access {
    pub store: Arc<Store>,
    pub clock: Arc<Clock>,
    pub users: Arc<Users>,
}

/// Who a request acts as.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Identity {
    Anonymous,
    User {
        id: UserId,
        /// The signature was in the query string.
        presigned: bool,
    },
}

impl Identity {
    pub fn signed(self) -> bool {
        self != Self::Anonymous
    }

    pub fn presigned(self) -> bool {
        matches!(self, Self::User { presigned: true, .. })
    }

    pub fn user(self) -> Option<UserId> {
        match self {
            Self::Anonymous => None,
            Self::User { id, .. } => Some(id),
        }
    }
}

/// The identity of a request, from the credentials s3s verified (`--auth`) or the [`Signed`]
/// marker left by a signature that was dropped unverified (default). Every authorization entry
/// point and every handler that needs to know who is asking goes through here.
pub fn identify<T>(users: &Users, req: &S3Request<T>) -> Identity {
    identify_with(
        req.credentials.as_ref().map(|c| users.resolve(Some(&c.access_key))),
        req.uri.query().is_some_and(|q| q.contains("X-Amz-Signature")),
        &req.extensions,
    )
}

/// [`identify`] for callers that cannot hold the request while borrowing its extensions mutably:
/// `verified` is the identity of the credentials s3s verified, if any.
fn identify_with(verified: Option<UserId>, presigned_query: bool, extensions: &http::Extensions) -> Identity {
    if let Some(id) = verified {
        return Identity::User {
            id,
            presigned: presigned_query,
        };
    }
    match extensions.get::<Signed>() {
        Some(s) => Identity::User {
            id: s.user,
            presigned: s.presigned,
        },
        None => Identity::Anonymous,
    }
}

pub fn denied() -> S3Error {
    S3Error::with_message(S3ErrorCode::AccessDenied, "Access Denied")
}

const EXPECTED_OWNER: &str = "x-amz-expected-bucket-owner";
const SOURCE_EXPECTED_OWNER: &str = "x-amz-source-expected-bucket-owner";

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

/// Whether the key of `upload_id` was started by `me`.
fn initiated_by(bucket: &Bucket, key: &str, query: &str, me: UserId) -> bool {
    query_param(query, "uploadId").is_some_and(|id| bucket.uploads.read().get(&(key.to_owned(), id)).is_some_and(|u| u.initiator == me))
}

/// Operations tied to a multipart upload that its initiator may always continue or abandon.
fn is_upload_op(op: &str) -> bool {
    matches!(
        op,
        "UploadPart" | "UploadPartCopy" | "CompleteMultipartUpload" | "AbortMultipartUpload" | "ListParts"
    )
}

impl Access {
    fn user_of(&self, who: Identity) -> Option<&crate::users::User> {
        who.user().map(|id| self.users.get(id))
    }

    /// Whether the bucket's ACL gives `who` `perm`. The owner never gets here.
    fn bucket_acl_allows(&self, bucket: &Bucket, who: Identity, perm: Perm, public: bool) -> bool {
        if !bucket.config.acls_enabled() {
            return false;
        }
        let acl = bucket.config.acl.read();
        let Some(acl) = acl.as_ref() else { return false };
        match self.user_of(who) {
            Some(u) => acl.allows_user(&u.canonical_id, perm, public),
            None => public && acl.allows_anonymous(perm),
        }
    }

    /// Whether `who` may do `perm` to an object that `owner` owns and `acl` describes.
    fn object_acl_allows(&self, acl: Option<&crate::acl::Acl>, owner: UserId, who: Identity, perm: Perm, public: bool) -> bool {
        match (acl, self.user_of(who)) {
            // The default ACL: full control for the owner, nothing for anybody else.
            (None, Some(_)) => who.user() == Some(owner),
            (None, None) => false,
            (Some(a), Some(u)) => {
                a.allows_user(&u.canonical_id, perm, public)
                    // The owner can always read and replace the ACL, whatever it says.
                    || (matches!(perm, Perm::ReadAcp | Perm::WriteAcp) && who.user() == Some(owner))
            }
            (Some(a), None) => public && a.allows_anonymous(perm),
        }
    }

    /// Whether the request may perform `op` on `key` (`None` for bucket-level operations).
    #[allow(clippy::too_many_arguments)]
    fn permit(
        &self,
        bucket: &Bucket,
        name: &str,
        op: &str,
        key: Option<&str>,
        version_id: Option<&str>,
        who: Identity,
        ctx: &Context<'_>,
    ) -> bool {
        let policy = bucket.config.policy.read().clone();
        let pab = bucket.config.public_access_block.read().unwrap_or_default();
        let owner = who.user() == Some(bucket.owner);
        let need = acl_need(op);
        let object_scope = key.is_some() && !matches!(need, Some((Scope::Bucket, _)));
        // The object itself, when the decision depends on it.
        let wants_object = key.is_some() && (object_scope || policy.as_ref().is_some_and(|p| p.uses_object_tags));
        let version = key.filter(|_| wants_object).and_then(|key| {
            let objects = bucket.objects.read();
            objects
                .get(key)
                .and_then(|e| match version_id {
                    Some(id) => e.find(id),
                    None => e.latest(),
                })
                .cloned()
        });
        let tags = version.as_ref().and_then(|v| v.obj.as_ref()).map(|o| o.tags.as_slice());
        let ctx = &Context {
            existing_tags: tags.filter(|_| policy.as_ref().is_some_and(|p| p.uses_object_tags)),
            ..*ctx
        };
        let mut by_policy = false;
        if let Some(p) = &policy {
            match p.evaluate(&action_of(op, version_id.is_some()), &arn(name, key), ctx) {
                Verdict::Deny if !(owner && is_policy_admin(op)) => return false,
                Verdict::Allow => by_policy = !(pab.restrict_public_buckets && p.public && !owner),
                _ => {}
            }
        }
        let public = !pab.ignore_public_acls;
        let initiator = || {
            is_upload_op(op)
                && who
                    .user()
                    .zip(key)
                    .is_some_and(|(me, key)| initiated_by(bucket, key, ctx.query, me))
        };
        if !object_scope {
            return owner || by_policy || need.is_some_and(|(_, perm)| self.bucket_acl_allows(bucket, who, perm, public)) || initiator();
        }
        let perm = need.map_or(Perm::FullControl, |(_, p)| p);
        let key = key.expect("object-scoped operations have a key");
        match version.as_ref().and_then(|v| v.obj.as_ref().map(|o| (v, o))) {
            Some((v, o)) => {
                // With ACLs disabled the bucket owner owns everything.
                let acls = bucket.config.acls_enabled();
                let object_owner = if acls { v.owner } else { bucket.owner };
                if object_owner == bucket.owner && (owner || by_policy) {
                    return true;
                }
                (acls && self.object_acl_allows(o.acl.as_deref(), object_owner, who, perm, public)) || initiator()
            }
            // Nothing to read: permission to list decides between 404 and 403.
            None => {
                owner
                    || by_policy
                    || (perm == Perm::Read
                        && (self.bucket_acl_allows(bucket, who, Perm::Read, public) || self.may_list(&policy, name, key, who, &pab, ctx)))
                    || initiator()
            }
        }
    }

    /// Whether a policy Allow for `s3:ListBucket` covers `key` as the listing prefix.
    fn may_list(
        &self,
        policy: &Option<Arc<crate::policy::Policy>>,
        name: &str,
        key: &str,
        who: Identity,
        pab: &crate::bucket_config::PublicAccessBlock,
        ctx: &Context<'_>,
    ) -> bool {
        let Some(p) = policy else { return false };
        let ctx = Context { prefix: Some(key), ..*ctx };
        p.evaluate("s3:ListBucket", &arn(name, None), &ctx) == Verdict::Allow && !(pab.restrict_public_buckets && p.public && !who.signed())
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

    /// 403 when `header` names an account other than the one that owns `bucket`. A bucket that
    /// does not exist belongs to nobody, so only the requester's own account id passes.
    fn expected_owner_ok(&self, headers: &http::HeaderMap, header: &str, owner: Option<UserId>, who: Identity) -> bool {
        let Some(v) = headers.get(header) else { return true };
        let want = owner.or(who.user()).map(|id| self.users.get(id));
        want.is_some_and(|u| v.as_bytes() == u.account_id.as_bytes())
    }
}

#[async_trait::async_trait]
impl S3Access for Access {
    async fn check(&self, cx: &mut S3AccessContext<'_>) -> S3Result<()> {
        let verified = cx.credentials().map(|c| self.users.resolve(Some(&c.access_key)));
        let identity = identify_with(
            verified,
            cx.uri().query().is_some_and(|q| q.contains("X-Amz-Signature")),
            cx.extensions_mut(),
        );
        let signed = identity.signed();
        let expected = cx.headers().contains_key(EXPECTED_OWNER) || cx.headers().contains_key(SOURCE_EXPECTED_OWNER);
        if signed && !expected && !self.users.multi() && !self.store.access.any_policy.load(Ordering::Relaxed) {
            return Ok(());
        }
        let peer = self.peer(cx.extensions_mut());
        let (name, key) = match cx.s3_path() {
            S3Path::Root => return if signed { Ok(()) } else { Err(denied()) },
            S3Path::Bucket { bucket } => (bucket.to_string(), None),
            S3Path::Object { bucket, key } => (bucket.to_string(), Some(key.to_string())),
        };
        let op = cx.s3_op().name();
        let bucket = self.store.bucket(&name);
        if !self.expected_owner_ok(cx.headers(), EXPECTED_OWNER, bucket.as_ref().map(|b| b.owner), identity) {
            return Err(denied());
        }
        // The name is taken or free, whoever asks: the operation tells those apart.
        if op == "CreateBucket" {
            return if signed { Ok(()) } else { Err(denied()) };
        }
        // Their keys are in the body or form; `delete_objects` / `post_object` below decide.
        if matches!(op, "DeleteObjects" | "PostObject") {
            return Ok(());
        }
        let Some(bucket) = bucket else {
            return Self::missing_bucket(signed, op);
        };
        let query = cx.uri().query().unwrap_or("");
        let version_id = key.as_ref().and_then(|_| query_param(query, "versionId"));
        let ctx = Context {
            headers: cx.headers(),
            query,
            now: self.clock.now(),
            user: self.user_of(identity),
            presigned: identity.presigned(),
            peer,
            prefix: None,
            existing_tags: None,
        };
        if !self.permit(&bucket, &name, op, key.as_deref(), version_id.as_deref(), identity, &ctx) {
            return Err(denied());
        }
        if matches!(op, "CopyObject" | "UploadPartCopy")
            && let Some(src) = ctx.headers.get("x-amz-copy-source").and_then(|v| v.to_str().ok())
            && let Ok(CopySource::Bucket {
                bucket: src_bucket,
                key: src_key,
                version_id,
            }) = CopySource::parse(src)
        {
            let sb = self.store.bucket(&src_bucket);
            if !self.expected_owner_ok(ctx.headers, SOURCE_EXPECTED_OWNER, sb.as_ref().map(|b| b.owner), identity) {
                return Err(denied());
            }
            if let Some(sb) = sb
                && !self.permit(&sb, &src_bucket, "GetObject", Some(&src_key), version_id.as_deref(), identity, &ctx)
            {
                return Err(denied());
            }
        }
        Ok(())
    }

    async fn delete_objects(&self, req: &mut S3Request<DeleteObjectsInput>) -> S3Result<()> {
        let identity = identify(&self.users, req);
        let signed = identity.signed();
        if signed && !self.users.multi() && !self.store.access.any_policy.load(Ordering::Relaxed) {
            return Ok(());
        }
        let Some(bucket) = self.store.bucket(&req.input.bucket) else {
            return Self::missing_bucket(signed, "DeleteObjects");
        };
        let query = req.uri.query().unwrap_or("");
        let peer = self.peer(&mut req.extensions);
        let ctx = Context {
            headers: &req.headers,
            query,
            now: self.clock.now(),
            user: self.user_of(identity),
            presigned: identity.presigned(),
            peer,
            prefix: None,
            existing_tags: None,
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
                    identity,
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
        let identity = identify(&self.users, req);
        let signed = identity.signed();
        if signed && !self.users.multi() && !self.store.access.any_policy.load(Ordering::Relaxed) {
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
            user: self.user_of(identity),
            presigned: identity.presigned(),
            peer,
            prefix: None,
            existing_tags: None,
        };
        if self.permit(&bucket, &req.input.bucket, "PostObject", Some(&req.input.key), None, identity, &ctx) {
            Ok(())
        } else {
            Err(denied())
        }
    }
}
