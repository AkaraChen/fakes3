//! Per-bucket configuration documents: ACL, ownership controls, public access block, policy,
//! CORS, lifecycle, website and notification settings.
//!
//! Every document is optional and defaults to "not configured", so a bucket that uses none of
//! them pays for nothing beyond the (empty) locks. The request path only looks at a document
//! when the request needs it; see `access.rs` for how the shared `AccessState` flag keeps signed
//! requests off the policy path until some bucket actually has a policy.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64};

use http::StatusCode;
use parking_lot::RwLock;
use s3s::dto::{CORSConfiguration, NotificationConfiguration, WebsiteConfiguration};
use s3s::{S3Error, S3ErrorCode};

use crate::acl::Acl;
use crate::lifecycle::Lifecycle;
use crate::policy::Policy;

/// `ObjectOwnership`. With `Enforced`, ACLs are disabled: they cannot be set and are ignored.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ownership {
    Enforced,
    Preferred,
    Writer,
}

impl Ownership {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "BucketOwnerEnforced" => Some(Self::Enforced),
            "BucketOwnerPreferred" => Some(Self::Preferred),
            "ObjectWriter" => Some(Self::Writer),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Enforced => "BucketOwnerEnforced",
            Self::Preferred => "BucketOwnerPreferred",
            Self::Writer => "ObjectWriter",
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct PublicAccessBlock {
    pub block_public_acls: bool,
    pub ignore_public_acls: bool,
    pub block_public_policy: bool,
    pub restrict_public_buckets: bool,
}

pub struct BucketConfig {
    pub acl: RwLock<Acl>,
    pub ownership: RwLock<Option<Ownership>>,
    pub public_access_block: RwLock<Option<PublicAccessBlock>>,
    pub policy: RwLock<Option<Arc<Policy>>>,
    pub cors: RwLock<Option<Arc<CORSConfiguration>>>,
    pub lifecycle: RwLock<Option<Arc<Lifecycle>>>,
    /// Mirrors `lifecycle.is_some()` so the per-request check is a single atomic load.
    pub has_lifecycle: AtomicBool,
    /// UTC day (days since the epoch, server clock) the lifecycle rules were last applied for;
    /// `i64::MIN` forces the next access to apply them.
    pub lifecycle_day: AtomicI64,
    pub website: RwLock<Option<WebsiteConfiguration>>,
    pub notification: RwLock<Option<NotificationConfiguration>>,
}

impl Default for BucketConfig {
    fn default() -> Self {
        Self {
            acl: RwLock::new(Acl::private()),
            ownership: RwLock::new(None),
            public_access_block: RwLock::new(None),
            policy: RwLock::new(None),
            cors: RwLock::new(None),
            lifecycle: RwLock::new(None),
            has_lifecycle: AtomicBool::new(false),
            lifecycle_day: AtomicI64::new(i64::MIN),
            website: RwLock::new(None),
            notification: RwLock::new(None),
        }
    }
}

impl BucketConfig {
    /// ACLs are in force unless the bucket owner enforced setting is on.
    pub fn acls_enabled(&self) -> bool {
        *self.ownership.read() != Some(Ownership::Enforced)
    }
}

/// Server-wide switches that let the request path skip bucket configuration lookups.
#[derive(Default)]
pub struct AccessState {
    /// Set once any bucket has been given a policy and never cleared: until then a signed
    /// request needs no authorization work at all.
    pub any_policy: AtomicBool,
    /// Set once a policy refers to `aws:SourceIp`; the accept loop then records client addresses.
    pub record_peers: AtomicBool,
    /// Set once any bucket has had lifecycle rules; until then responses need no expiration lookup.
    pub any_lifecycle: AtomicBool,
}

/// An error with a code s3s has no variant for.
pub fn custom_error(code: &'static str, status: StatusCode, msg: &'static str) -> S3Error {
    let mut e = S3Error::with_message(S3ErrorCode::Custom(code.into()), msg);
    e.set_status_code(status);
    e
}
