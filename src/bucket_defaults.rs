//! Which settings a new bucket starts with.
//!
//! AWS changed the defaults of new buckets twice, and fakes3 keeps the older ones unless asked
//! otherwise, so that "put a public policy, then read anonymously" works out of the box:
//!
//! * [`BucketDefaults::Legacy`]: ACLs enabled (no ownership controls), no Block Public Access, no
//!   default encryption configuration.
//! * [`BucketDefaults::Aws`]: what AWS documents for new buckets today. Object Ownership is
//!   `BucketOwnerEnforced` (ACLs disabled, since April 2023) and all four Block Public Access
//!   settings are on (April 2023); new objects are encrypted with SSE-S3 (January 2023, see
//!   [`BucketDefaults::default_encryption`]) and SSE-C is blocked (April 2026).
//!
//! The mode only decides the state of a bucket at creation. Afterwards the bucket's own
//! documents rule, so snapshots, restores and resets never consult it.

use std::fmt;
use std::str::FromStr;
use std::time::SystemTime;

use s3s::dto::{BlockedEncryptionTypes, EncryptionType, ServerSideEncryption};

use crate::bucket_config::{BucketConfig, Ownership, PublicAccessBlock};
use crate::objext::EncryptionRule;
use crate::store::Bucket;
use crate::users::UserId;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BucketDefaults {
    #[default]
    Legacy,
    Aws,
}

impl BucketDefaults {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::Aws => "aws",
        }
    }

    /// An empty bucket in the state CreateBucket leaves it before the request's own options
    /// (`x-amz-object-ownership`, ACL headers, Object Lock) are applied.
    pub(crate) fn new_bucket(self, now: SystemTime, owner: UserId) -> Bucket {
        let bucket = Bucket::new(now, owner);
        self.apply(&bucket.config);
        bucket.cfg.set_encryption(self.default_encryption());
        bucket
    }

    /// Sets the configuration documents a freshly created bucket has before the request's own
    /// options (`x-amz-object-ownership`, ACL headers) are applied.
    fn apply(self, config: &BucketConfig) {
        if self == Self::Aws {
            *config.ownership.write() = Some(Ownership::Enforced);
            *config.public_access_block.write() = Some(PublicAccessBlock {
                block_public_acls: true,
                ignore_public_acls: true,
                block_public_policy: true,
                restrict_public_buckets: true,
            });
        }
    }

    /// The default encryption configuration of a new bucket, and what DeleteBucketEncryption
    /// goes back to. In `aws` mode:
    ///
    /// * SSE-S3, which AWS applies to every bucket and no longer lets anyone remove (January
    ///   2023): GetBucketEncryption reports `AES256`, and PUT, GET and HEAD carry
    ///   `x-amz-server-side-encryption: AES256`;
    /// * SSE-C blocked (April 2026): writes that ask for it fail with 403 `AccessDenied` until
    ///   PutBucketEncryption sets `BlockedEncryptionTypes` to `NONE`.
    ///
    /// <https://docs.aws.amazon.com/AmazonS3/latest/userguide/default-s3-c-encryption-setting-faq.html>
    ///
    /// In `legacy` mode there is none.
    pub(crate) fn default_encryption(self) -> Option<EncryptionRule> {
        (self == Self::Aws).then(|| EncryptionRule {
            algorithm: ServerSideEncryption::from_static(ServerSideEncryption::AES256),
            kms_key_id: None,
            bucket_key: Some(false),
            blocked: Some(BlockedEncryptionTypes {
                encryption_type: Some(vec![EncryptionType::from_static(EncryptionType::SSE_C)]),
            }),
        })
    }
}

impl fmt::Display for BucketDefaults {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for BucketDefaults {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        match s.to_ascii_lowercase().as_str() {
            "legacy" => Ok(Self::Legacy),
            "aws" => Ok(Self::Aws),
            _ => Err(format!("unknown bucket defaults {s:?}: expected `legacy` or `aws`")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_prints() {
        assert_eq!("aws".parse(), Ok(BucketDefaults::Aws));
        assert_eq!("Legacy".parse(), Ok(BucketDefaults::Legacy));
        assert!("2023".parse::<BucketDefaults>().is_err());
        assert_eq!(BucketDefaults::Aws.to_string(), "aws");
    }

    #[test]
    fn aws_defaults_disable_acls_and_block_public_access() {
        let legacy = BucketConfig::default();
        BucketDefaults::Legacy.apply(&legacy);
        assert!(legacy.acls_enabled() && legacy.public_access_block.read().is_none());

        let aws = BucketConfig::default();
        BucketDefaults::Aws.apply(&aws);
        assert!(!aws.acls_enabled());
        let pab = aws.public_access_block.read().unwrap();
        assert!(pab.block_public_acls && pab.ignore_public_acls && pab.block_public_policy && pab.restrict_public_buckets);
        assert!(BucketDefaults::Aws.default_encryption().is_some() && BucketDefaults::Legacy.default_encryption().is_none());
    }
}
