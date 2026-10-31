//! Access control lists: canned ACLs, `x-amz-grant-*` headers and `AccessControlPolicy` bodies.
//!
//! fakes3 has a single account, so the only grantees that matter for authorization are the
//! predefined groups (`AllUsers` is what makes anonymous access work). Other canonical users are
//! stored and reported but never match a request.
//!
//! An object without an explicit ACL is `private`; the bucket's ACL starts out `private` too.

use s3s::dto::{AccessControlPolicy, Grant, Grantee as DtoGrantee, Owner, Permission, Type};
use s3s::{S3Error, S3Result, s3_error};

use crate::bucket_config::BucketConfig;

pub const OWNER_ID: &str = "fakes3";

const ALL_USERS: &str = "http://acs.amazonaws.com/groups/global/AllUsers";
const AUTHENTICATED_USERS: &str = "http://acs.amazonaws.com/groups/global/AuthenticatedUsers";
const LOG_DELIVERY: &str = "http://acs.amazonaws.com/groups/s3/LogDelivery";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Group {
    AllUsers,
    AuthenticatedUsers,
    LogDelivery,
}

impl Group {
    fn uri(self) -> &'static str {
        match self {
            Self::AllUsers => ALL_USERS,
            Self::AuthenticatedUsers => AUTHENTICATED_USERS,
            Self::LogDelivery => LOG_DELIVERY,
        }
    }

    fn from_uri(uri: &str) -> Option<Self> {
        [Self::AllUsers, Self::AuthenticatedUsers, Self::LogDelivery]
            .into_iter()
            .find(|g| g.uri() == uri)
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Grantee {
    User(String),
    Group(Group),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Perm {
    Read,
    Write,
    ReadAcp,
    WriteAcp,
    FullControl,
}

impl Perm {
    fn name(self) -> &'static str {
        match self {
            Self::Read => Permission::READ,
            Self::Write => Permission::WRITE,
            Self::ReadAcp => Permission::READ_ACP,
            Self::WriteAcp => Permission::WRITE_ACP,
            Self::FullControl => Permission::FULL_CONTROL,
        }
    }

    fn parse(s: &str) -> Option<Self> {
        [Self::Read, Self::Write, Self::ReadAcp, Self::WriteAcp, Self::FullControl]
            .into_iter()
            .find(|p| p.name() == s)
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Acl {
    pub grants: Vec<(Grantee, Perm)>,
}

impl Acl {
    /// The owner has full control and nobody else has anything.
    pub fn private() -> Self {
        Self {
            grants: vec![(Grantee::User(OWNER_ID.into()), Perm::FullControl)],
        }
    }

    fn with(extra: &[(Group, Perm)]) -> Self {
        let mut acl = Self::private();
        acl.grants.extend(extra.iter().map(|(g, p)| (Grantee::Group(*g), *p)));
        acl
    }

    /// Whether the ACL grants `perm` to everyone, which is what lets an anonymous request in.
    pub fn allows_anonymous(&self, perm: Perm) -> bool {
        self.grants
            .iter()
            .any(|(g, p)| *g == Grantee::Group(Group::AllUsers) && (*p == perm || *p == Perm::FullControl))
    }

    /// Whether any grant is to `AllUsers` or `AuthenticatedUsers`, i.e. what Block Public Access calls public.
    pub fn is_public(&self) -> bool {
        self.grants
            .iter()
            .any(|(g, _)| matches!(g, Grantee::Group(Group::AllUsers | Group::AuthenticatedUsers)))
    }

    pub fn to_dto(&self) -> Vec<Grant> {
        self.grants
            .iter()
            .map(|(g, p)| Grant {
                grantee: Some(match g {
                    Grantee::User(id) => DtoGrantee {
                        display_name: (id == OWNER_ID).then(|| OWNER_ID.to_owned()),
                        email_address: None,
                        id: Some(id.clone()),
                        type_: Type::from_static(Type::CANONICAL_USER),
                        uri: None,
                    },
                    Grantee::Group(group) => DtoGrantee {
                        display_name: None,
                        email_address: None,
                        id: None,
                        type_: Type::from_static(Type::GROUP),
                        uri: Some(group.uri().to_owned()),
                    },
                }),
                permission: Some(Permission::from_static(p.name())),
            })
            .collect()
    }

    pub fn from_dto(policy: AccessControlPolicy) -> S3Result<Self> {
        let malformed = || {
            s3_error!(
                MalformedACLError,
                "The XML you provided was not well-formed or did not validate against our published schema"
            )
        };
        let mut grants = Vec::new();
        for g in policy.grants.unwrap_or_default() {
            let (Some(grantee), Some(perm)) = (g.grantee, g.permission) else {
                return Err(malformed());
            };
            let perm = Perm::parse(perm.as_str()).ok_or_else(malformed)?;
            let grantee = match grantee.type_.as_str() {
                Type::CANONICAL_USER => Grantee::User(valid_id(&grantee.id.ok_or_else(malformed)?)?),
                Type::GROUP => Grantee::Group(valid_group(&grantee.uri.ok_or_else(malformed)?)?),
                Type::AMAZON_CUSTOMER_BY_EMAIL => return Err(unresolvable_email()),
                _ => return Err(malformed()),
            };
            grants.push((grantee, perm));
        }
        Ok(Self { grants })
    }

    pub fn owner() -> Owner {
        Owner {
            display_name: Some(OWNER_ID.into()),
            id: Some(OWNER_ID.into()),
        }
    }
}

fn valid_id(id: &str) -> S3Result<String> {
    if id == OWNER_ID || (id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit())) {
        Ok(id.to_owned())
    } else {
        Err(s3_error!(InvalidArgument, "Invalid id"))
    }
}

fn valid_group(uri: &str) -> S3Result<Group> {
    Group::from_uri(uri).ok_or_else(|| s3_error!(InvalidArgument, "Invalid group uri"))
}

fn unresolvable_email() -> S3Error {
    s3_error!(
        UnresolvableGrantByEmailAddress,
        "The email address you provided does not match any account on record."
    )
}

/// The ACL-related request headers of an operation, borrowed from its input.
#[derive(Default)]
pub struct AclArgs<'a> {
    pub canned: Option<&'a str>,
    pub full_control: Option<&'a str>,
    pub read: Option<&'a str>,
    pub read_acp: Option<&'a str>,
    pub write: Option<&'a str>,
    pub write_acp: Option<&'a str>,
}

/// Collects the ACL headers of an input. Objects have no `x-amz-grant-write`; pass `bucket` for
/// inputs that do.
macro_rules! acl_args {
    ($i:expr) => {{
        let i = $i;
        $crate::acl::AclArgs {
            canned: i.acl.as_ref().map(|a| a.as_str()),
            full_control: i.grant_full_control.as_deref(),
            read: i.grant_read.as_deref(),
            read_acp: i.grant_read_acp.as_deref(),
            write_acp: i.grant_write_acp.as_deref(),
            ..Default::default()
        }
    }};
    ($i:expr, bucket) => {{
        let i = $i;
        $crate::acl::AclArgs {
            write: i.grant_write.as_deref(),
            ..$crate::acl::acl_args!(i)
        }
    }};
}
pub(crate) use acl_args;

impl AclArgs<'_> {
    pub fn is_empty(&self) -> bool {
        self.canned.is_none()
            && self.full_control.is_none()
            && self.read.is_none()
            && self.read_acp.is_none()
            && self.write.is_none()
            && self.write_acp.is_none()
    }

    fn grant_headers(&self) -> [(&'static str, Option<&str>, Perm); 5] {
        [
            ("x-amz-grant-full-control", self.full_control, Perm::FullControl),
            ("x-amz-grant-read", self.read, Perm::Read),
            ("x-amz-grant-read-acp", self.read_acp, Perm::ReadAcp),
            ("x-amz-grant-write", self.write, Perm::Write),
            ("x-amz-grant-write-acp", self.write_acp, Perm::WriteAcp),
        ]
    }

    /// Builds the ACL these headers describe, or `None` when there are none.
    pub fn to_acl(&self, bucket: bool) -> S3Result<Option<Acl>> {
        let grants_given = self.grant_headers().iter().any(|(_, v, _)| v.is_some());
        match (self.canned, grants_given) {
            (None, false) => Ok(None),
            (Some(_), true) => Err(s3_error!(
                InvalidRequest,
                "Specifying both Canned ACLs and Header Grants is not allowed"
            )),
            (Some(c), false) => canned(c, bucket).map(Some),
            (None, true) => {
                let mut acl = Acl { grants: Vec::new() };
                for (header, value, perm) in self.grant_headers() {
                    for grantee in value.into_iter().flat_map(split_grantees) {
                        acl.grants.push((parse_grantee(header, grantee)?, perm));
                    }
                }
                Ok(Some(acl))
            }
        }
    }
}

fn canned(name: &str, bucket: bool) -> S3Result<Acl> {
    Ok(match name {
        "private" => Acl::private(),
        "public-read" => Acl::with(&[(Group::AllUsers, Perm::Read)]),
        "public-read-write" => Acl::with(&[(Group::AllUsers, Perm::Read), (Group::AllUsers, Perm::Write)]),
        "authenticated-read" => Acl::with(&[(Group::AuthenticatedUsers, Perm::Read)]),
        "log-delivery-write" if bucket => Acl::with(&[(Group::LogDelivery, Perm::Write), (Group::LogDelivery, Perm::ReadAcp)]),
        // The bucket owner is the object owner here, so these add nothing.
        "aws-exec-read" | "bucket-owner-read" | "bucket-owner-full-control" if !bucket => Acl::private(),
        _ => return Err(s3_error!(InvalidArgument, "Invalid canned ACL")),
    })
}

/// Splits `id="a", uri="b"` at the commas that are not inside quotes.
fn split_grantees(v: &str) -> impl Iterator<Item = &str> {
    let mut in_quotes = false;
    let mut start = 0;
    let mut out = Vec::new();
    for (i, c) in v.char_indices() {
        match c {
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => {
                out.push(&v[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&v[start..]);
    out.into_iter().map(str::trim).filter(|s| !s.is_empty())
}

fn parse_grantee(header: &'static str, s: &str) -> S3Result<Grantee> {
    let bad = || {
        let mut e = s3_error!(InvalidArgument, "Argument format not recognized");
        e.set_headers({
            let mut h = http::HeaderMap::new();
            h.insert("x-amz-argument-name", http::HeaderValue::from_static(header));
            h
        });
        e
    };
    let (key, value) = s.split_once('=').ok_or_else(bad)?;
    let value = value.trim().trim_matches('"');
    match key.trim().to_ascii_lowercase().as_str() {
        "id" => valid_id(value).map(Grantee::User),
        "uri" => valid_group(value).map(Grantee::Group),
        "emailaddress" => Err(unresolvable_email()),
        _ => Err(bad()),
    }
}

/// Validates the ACL a write wants to set against the bucket's ownership controls and Block
/// Public Access settings. Returns the ACL to store (`None` means the default, `private`).
pub fn resolve(cfg: &BucketConfig, args: &AclArgs<'_>, bucket: bool) -> S3Result<Option<Acl>> {
    if args.is_empty() {
        return Ok(None);
    }
    if !cfg.acls_enabled() {
        // With ACLs disabled, only the owner-full-control canned ACL is accepted (and ignored).
        return if args.canned == Some("bucket-owner-full-control") && args.grant_headers().iter().all(|(_, v, _)| v.is_none()) {
            Ok(None)
        } else {
            Err(s3_error!(AccessControlListNotSupported, "The bucket does not allow ACLs"))
        };
    }
    let acl = args.to_acl(bucket)?;
    if let Some(a) = &acl
        && a.is_public()
        && cfg.public_access_block.read().is_some_and(|p| p.block_public_acls)
    {
        return Err(S3Error::with_message(s3s::S3ErrorCode::AccessDenied, "Access Denied"));
    }
    Ok(acl.filter(|a| *a != Acl::private()))
}

/// CreateBucket's variant: the ownership setting comes from the same request.
pub fn resolve_for_new_bucket(cfg: &BucketConfig, args: &AclArgs<'_>) -> S3Result<Option<Acl>> {
    resolve(cfg, args, true).map_err(|e| {
        if *e.code() == s3s::S3ErrorCode::AccessControlListNotSupported {
            s3_error!(
                InvalidBucketAclWithObjectOwnership,
                "Bucket cannot have ACLs set with ObjectOwnership's BucketOwnerEnforced setting"
            )
        } else {
            e
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(canned: Option<&'static str>) -> AclArgs<'static> {
        AclArgs {
            canned,
            ..Default::default()
        }
    }

    #[test]
    fn canned_acls() {
        let a = args(Some("public-read")).to_acl(false).unwrap().unwrap();
        assert!(a.allows_anonymous(Perm::Read) && !a.allows_anonymous(Perm::Write) && a.is_public());
        let a = args(Some("public-read-write")).to_acl(true).unwrap().unwrap();
        assert!(a.allows_anonymous(Perm::Write));
        assert!(args(Some("bucket-owner-full-control")).to_acl(true).is_err());
        assert!(args(Some("log-delivery-write")).to_acl(false).is_err());
        assert!(args(Some("nonsense")).to_acl(false).is_err());
    }

    #[test]
    fn grant_headers() {
        let a = AclArgs {
            read: Some(r#"uri="http://acs.amazonaws.com/groups/global/AllUsers", id="fakes3""#),
            write_acp: Some(r#"id="fakes3""#),
            ..Default::default()
        };
        let acl = a.to_acl(true).unwrap().unwrap();
        assert_eq!(acl.grants.len(), 3);
        assert!(acl.allows_anonymous(Perm::Read) && !acl.allows_anonymous(Perm::WriteAcp));
        let both = AclArgs {
            canned: Some("private"),
            read: Some(r#"id="fakes3""#),
            ..Default::default()
        };
        assert!(both.to_acl(true).is_err());
        let bad = AclArgs {
            read: Some(r#"id="nobody""#),
            ..Default::default()
        };
        assert!(bad.to_acl(true).is_err());
    }

    #[test]
    fn enforced_ownership_disables_acls() {
        let cfg = BucketConfig::default();
        *cfg.ownership.write() = Some(crate::bucket_config::Ownership::Enforced);
        assert!(resolve(&cfg, &args(Some("public-read")), false).is_err());
        assert!(resolve(&cfg, &args(Some("bucket-owner-full-control")), false).unwrap().is_none());
        assert!(resolve(&cfg, &args(None), false).unwrap().is_none());
    }

    #[test]
    fn block_public_acls_rejects_public_grants() {
        let cfg = BucketConfig::default();
        *cfg.public_access_block.write() = Some(crate::bucket_config::PublicAccessBlock {
            block_public_acls: true,
            ..Default::default()
        });
        assert!(resolve(&cfg, &args(Some("public-read")), true).is_err());
        assert!(resolve(&cfg, &args(Some("private")), true).unwrap().is_none());
    }

    #[test]
    fn dto_round_trip() {
        let acl = args(Some("public-read")).to_acl(false).unwrap().unwrap();
        let back = Acl::from_dto(AccessControlPolicy {
            grants: Some(acl.to_dto()),
            owner: Some(Acl::owner()),
        })
        .unwrap();
        assert_eq!(acl, back);
    }
}
