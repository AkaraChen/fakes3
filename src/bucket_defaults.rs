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
//!   [`BucketDefaults::sse_s3_by_default`]).
//!
//! The mode only decides the state of a bucket at creation. Afterwards the bucket's own
//! documents rule, so snapshots, restores and resets never consult it.

use std::fmt;
use std::str::FromStr;

use crate::bucket_config::{BucketConfig, Ownership, PublicAccessBlock};

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

    /// Sets the configuration documents a freshly created bucket has before the request's own
    /// options (`x-amz-object-ownership`, ACL headers) are applied.
    pub(crate) fn apply(self, config: &BucketConfig) {
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

    /// Whether a bucket without an encryption configuration encrypts new objects with SSE-S3:
    /// GetBucketEncryption then reports `AES256`, and PUT, GET and HEAD carry
    /// `x-amz-server-side-encryption: AES256`. The server-side encryption code asks this instead
    /// of checking the mode itself.
    #[allow(dead_code)] // Nothing consults it until the SSE code (feat/s3-object-semantics) lands.
    pub(crate) fn sse_s3_by_default(self) -> bool {
        self == Self::Aws
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
    }
}
