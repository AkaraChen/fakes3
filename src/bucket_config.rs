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
use crate::archive::ItConfigs;
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
    /// `None` is the default `private` ACL (full control for the bucket owner).
    pub acl: RwLock<Option<Acl>>,
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
    /// Intelligent-Tiering configurations by id; they decide when objects reach an archive tier.
    pub intelligent_tiering: RwLock<ItConfigs>,
}

impl Default for BucketConfig {
    fn default() -> Self {
        Self {
            acl: RwLock::new(None),
            ownership: RwLock::new(None),
            public_access_block: RwLock::new(None),
            policy: RwLock::new(None),
            cors: RwLock::new(None),
            lifecycle: RwLock::new(None),
            has_lifecycle: AtomicBool::new(false),
            lifecycle_day: AtomicI64::new(i64::MIN),
            website: RwLock::new(None),
            notification: RwLock::new(None),
            intelligent_tiering: RwLock::default(),
        }
    }
}

/// A copy of a bucket's configuration documents, for control-plane snapshots. The documents are
/// small and mostly shared (`Arc`), so copying them is cheap.
#[derive(Clone)]
pub struct ConfigSnapshot {
    acl: Option<Acl>,
    ownership: Option<Ownership>,
    public_access_block: Option<PublicAccessBlock>,
    policy: Option<Arc<Policy>>,
    cors: Option<Arc<CORSConfiguration>>,
    lifecycle: Option<Arc<Lifecycle>>,
    website: Option<WebsiteConfiguration>,
    notification: Option<NotificationConfiguration>,
    intelligent_tiering: ItConfigs,
}

impl BucketConfig {
    pub fn snapshot(&self) -> ConfigSnapshot {
        // Listing every field makes adding one to `BucketConfig` a compile error here.
        let Self {
            acl,
            ownership,
            public_access_block,
            policy,
            cors,
            lifecycle,
            has_lifecycle: _,
            lifecycle_day: _,
            website,
            notification,
            intelligent_tiering,
        } = self;
        ConfigSnapshot {
            acl: acl.read().clone(),
            ownership: *ownership.read(),
            public_access_block: *public_access_block.read(),
            policy: policy.read().clone(),
            cors: cors.read().clone(),
            lifecycle: lifecycle.read().clone(),
            website: website.read().clone(),
            notification: notification.read().clone(),
            intelligent_tiering: intelligent_tiering.read().clone(),
        }
    }

    pub fn restore(s: &ConfigSnapshot) -> Self {
        let c = Self::default();
        *c.acl.write() = s.acl.clone();
        *c.ownership.write() = s.ownership;
        *c.public_access_block.write() = s.public_access_block;
        *c.policy.write() = s.policy.clone();
        *c.cors.write() = s.cors.clone();
        // The restored rules apply at the next access, as when they were first put.
        c.has_lifecycle.store(s.lifecycle.is_some(), std::sync::atomic::Ordering::Release);
        *c.lifecycle.write() = s.lifecycle.clone();
        *c.website.write() = s.website.clone();
        *c.notification.write() = s.notification.clone();
        *c.intelligent_tiering.write() = s.intelligent_tiering.clone();
        c
    }

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
