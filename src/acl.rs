//! Access control lists: canned ACLs, `x-amz-grant-*` headers and `AccessControlPolicy` bodies.
//!
//! Grantees are canonical users (an identity's canonical id, see [`crate::users`]; ids that belong
//! to nobody are stored and reported but never match a request) and the predefined groups:
//! `AllUsers` is what makes anonymous access work, `AuthenticatedUsers` is any signed request.
//!
//! An object without an explicit ACL is `private`, i.e. its owner has full control; the bucket's
//! ACL starts out `private` too. Neither stores the owner's id until the ACL is changed.

use s3s::dto::{AccessControlPolicy, Grant, Grantee as DtoGrantee, Permission, Type};
use s3s::{S3Error, S3Result, s3_error};

use crate::bucket_config::BucketConfig;
use crate::users::{UserId, Users};

/// The accounts an ACL is being built or read for.
#[derive(Clone, Copy)]
pub struct Parties<'a> {
    pub users: &'a Users,
    /// Owner of the resource the ACL belongs to.
    pub owner: UserId,
    pub bucket_owner: UserId,
}

impl Parties<'_> {
    fn owner_id(&self) -> &str {
        &self.users.get(self.owner).canonical_id
    }

    fn bucket_owner_id(&self) -> &str {
        &self.users.get(self.bucket_owner).canonical_id
    }
}

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
    /// The owner (a canonical id) has full control and nobody else has anything.
    pub fn private(owner: &str) -> Self {
        Self {
            grants: vec![(Grantee::User(owner.into()), Perm::FullControl)],
        }
    }

    fn with(owner: &str, extra: &[(Group, Perm)]) -> Self {
        let mut acl = Self::private(owner);
        acl.grants.extend(extra.iter().map(|(g, p)| (Grantee::Group(*g), *p)));
        acl
    }

    /// Whether the ACL grants `perm` to everyone, which is what lets an anonymous request in.
    pub fn allows_anonymous(&self, perm: Perm) -> bool {
        self.grants
            .iter()
            .any(|(g, p)| *g == Grantee::Group(Group::AllUsers) && (*p == perm || *p == Perm::FullControl))
    }

    /// Whether the ACL gives the signed-in account `canonical_id` `perm`: a grant to it, or to
    /// `AuthenticatedUsers` / `AllUsers` unless `public` is false (`IgnorePublicAcls`).
    pub fn allows_user(&self, canonical_id: &str, perm: Perm, public: bool) -> bool {
        self.grants.iter().any(|(g, p)| {
            (*p == perm || *p == Perm::FullControl)
                && match g {
                    Grantee::User(id) => id == canonical_id,
                    Grantee::Group(Group::AllUsers | Group::AuthenticatedUsers) => public,
                    Grantee::Group(Group::LogDelivery) => false,
                }
        })
    }

    /// Whether any grant is to `AllUsers` or `AuthenticatedUsers`, i.e. what Block Public Access calls public.
    pub fn is_public(&self) -> bool {
        self.grants
            .iter()
            .any(|(g, _)| matches!(g, Grantee::Group(Group::AllUsers | Group::AuthenticatedUsers)))
    }

    pub fn to_dto(&self, users: &Users) -> Vec<Grant> {
        self.grants
            .iter()
            .map(|(g, p)| Grant {
                grantee: Some(match g {
                    Grantee::User(id) => DtoGrantee {
                        display_name: users.by_canonical(id).map(|(_, u)| u.name.clone()),
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

    pub fn from_dto(policy: AccessControlPolicy, users: &Users) -> S3Result<Self> {
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
                Type::CANONICAL_USER => Grantee::User(valid_id(&grantee.id.ok_or_else(malformed)?, users)?),
                Type::GROUP => Grantee::Group(valid_group(&grantee.uri.ok_or_else(malformed)?)?),
                Type::AMAZON_CUSTOMER_BY_EMAIL => return Err(unresolvable_email()),
                _ => return Err(malformed()),
            };
            grants.push((grantee, perm));
        }
        Ok(Self { grants })
    }
}

/// A canonical id that is some configured identity's or looks like one.
fn valid_id(id: &str, users: &Users) -> S3Result<String> {
    if users.by_canonical(id).is_some() || (id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit())) {
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
    pub fn to_acl(&self, bucket: bool, who: Parties<'_>) -> S3Result<Option<Acl>> {
        let grants_given = self.grant_headers().iter().any(|(_, v, _)| v.is_some());
        match (self.canned, grants_given) {
            (None, false) => Ok(None),
            (Some(_), true) => Err(s3_error!(
                InvalidRequest,
                "Specifying both Canned ACLs and Header Grants is not allowed"
            )),
            (Some(c), false) => canned(c, bucket, who).map(Some),
            (None, true) => {
                let mut acl = Acl { grants: Vec::new() };
                for (header, value, perm) in self.grant_headers() {
                    for grantee in value.into_iter().flat_map(split_grantees) {
                        acl.grants.push((parse_grantee(header, grantee, who.users)?, perm));
                    }
                }
                Ok(Some(acl))
            }
        }
    }
}

fn canned(name: &str, bucket: bool, who: Parties<'_>) -> S3Result<Acl> {
    let owner = who.owner_id();
    Ok(match name {
        "private" => Acl::private(owner),
        "public-read" => Acl::with(owner, &[(Group::AllUsers, Perm::Read)]),
        "public-read-write" => Acl::with(owner, &[(Group::AllUsers, Perm::Read), (Group::AllUsers, Perm::Write)]),
        "authenticated-read" => Acl::with(owner, &[(Group::AuthenticatedUsers, Perm::Read)]),
        "log-delivery-write" if bucket => Acl::with(owner, &[(Group::LogDelivery, Perm::Write), (Group::LogDelivery, Perm::ReadAcp)]),
        // These two give the bucket owner a grant on an object somebody else owns.
        "bucket-owner-read" | "bucket-owner-full-control" if !bucket => {
            let mut acl = Acl::private(owner);
            if who.owner != who.bucket_owner {
                let perm = if name == "bucket-owner-read" {
                    Perm::Read
                } else {
                    Perm::FullControl
                };
                acl.grants.push((Grantee::User(who.bucket_owner_id().into()), perm));
            }
            acl
        }
        "aws-exec-read" if !bucket => Acl::private(owner),
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

fn parse_grantee(header: &'static str, s: &str, users: &Users) -> S3Result<Grantee> {
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
        "id" => valid_id(value, users).map(Grantee::User),
        "uri" => valid_group(value).map(Grantee::Group),
        "emailaddress" => Err(unresolvable_email()),
        _ => Err(bad()),
    }
}

/// Validates the ACL a write wants to set against the bucket's ownership controls and Block
/// Public Access settings. Returns the ACL to store (`None` means the default, `private`).
pub fn resolve(cfg: &BucketConfig, args: &AclArgs<'_>, bucket: bool, who: Parties<'_>) -> S3Result<Option<Acl>> {
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
    let acl = args.to_acl(bucket, who)?;
    if let Some(a) = &acl
        && a.is_public()
        && cfg.public_access_block.read().is_some_and(|p| p.block_public_acls)
    {
        return Err(S3Error::with_message(s3s::S3ErrorCode::AccessDenied, "Access Denied"));
    }
    Ok(acl.filter(|a| *a != Acl::private(who.owner_id())))
}

/// CreateBucket's variant: the ownership setting comes from the same request. With ACLs disabled
/// the bucket ACL must give full control only to the owner, which is what `private` is; anything
/// else is `InvalidBucketAclWithObjectOwnership`.
/// <https://docs.aws.amazon.com/AmazonS3/latest/userguide/object-ownership-error-responses.html>
pub fn resolve_for_new_bucket(cfg: &BucketConfig, args: &AclArgs<'_>, who: Parties<'_>) -> S3Result<Option<Acl>> {
    resolve(cfg, args, true, who).or_else(|e| {
        if *e.code() != s3s::S3ErrorCode::AccessControlListNotSupported {
            return Err(e);
        }
        if args
            .to_acl(true, who)
            .is_ok_and(|a| a.is_none_or(|a| a == Acl::private(who.owner_id())))
        {
            return Ok(None);
        }
        Err(s3_error!(
            InvalidBucketAclWithObjectOwnership,
            "Bucket cannot have ACLs set with ObjectOwnership's BucketOwnerEnforced setting"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::users::UserConfig;

    fn users() -> &'static Users {
        let alt: UserConfig = "alt:ALTKEY:ALTSECRET:111122223333:cafe".parse().unwrap();
        Box::leak(Box::new(Users::new("fakes3", "fakes3secret", &[alt]).unwrap()))
    }

    fn alt() -> UserId {
        users().resolve(Some("ALTKEY"))
    }

    /// The default identity owns everything.
    fn me() -> Parties<'static> {
        Parties {
            users: users(),
            owner: UserId::DEFAULT,
            bucket_owner: UserId::DEFAULT,
        }
    }

    fn args(canned: Option<&'static str>) -> AclArgs<'static> {
        AclArgs {
            canned,
            ..Default::default()
        }
    }

    #[test]
    fn canned_acls() {
        let a = args(Some("public-read")).to_acl(false, me()).unwrap().unwrap();
        assert!(a.allows_anonymous(Perm::Read) && !a.allows_anonymous(Perm::Write) && a.is_public());
        let a = args(Some("public-read-write")).to_acl(true, me()).unwrap().unwrap();
        assert!(a.allows_anonymous(Perm::Write));
        assert!(args(Some("bucket-owner-full-control")).to_acl(true, me()).is_err());
        assert!(args(Some("log-delivery-write")).to_acl(false, me()).is_err());
        assert!(args(Some("nonsense")).to_acl(false, me()).is_err());
    }

    #[test]
    fn bucket_owner_canned_acls_grant_the_bucket_owner() {
        let uploader = Parties { owner: alt(), ..me() };
        let full = args(Some("bucket-owner-full-control")).to_acl(false, uploader).unwrap().unwrap();
        assert_eq!(
            full.grants,
            [
                (Grantee::User("cafe".into()), Perm::FullControl),
                (Grantee::User("fakes3".into()), Perm::FullControl)
            ]
        );
        let read = args(Some("bucket-owner-read")).to_acl(false, uploader).unwrap().unwrap();
        assert_eq!(read.grants[1], (Grantee::User("fakes3".into()), Perm::Read));
        // Nothing to add when the uploader is the bucket owner.
        let own = args(Some("bucket-owner-full-control")).to_acl(false, me()).unwrap().unwrap();
        assert_eq!(own, Acl::private("fakes3"));
    }

    #[test]
    fn allows_the_grantee_and_the_groups() {
        let acl = Acl {
            grants: vec![
                (Grantee::User("fakes3".into()), Perm::FullControl),
                (Grantee::User("cafe".into()), Perm::Read),
                (Grantee::Group(Group::AuthenticatedUsers), Perm::ReadAcp),
            ],
        };
        assert!(acl.allows_user("cafe", Perm::Read, true) && !acl.allows_user("cafe", Perm::Write, true));
        assert!(acl.allows_user("cafe", Perm::ReadAcp, true));
        assert!(!acl.allows_user("cafe", Perm::ReadAcp, false), "public grants ignored");
        assert!(acl.allows_user("fakes3", Perm::Write, false));
        assert!(!acl.allows_user("other", Perm::Read, true));
    }

    #[test]
    fn grant_headers() {
        let a = AclArgs {
            read: Some(r#"uri="http://acs.amazonaws.com/groups/global/AllUsers", id="fakes3""#),
            write_acp: Some(r#"id="cafe""#),
            ..Default::default()
        };
        let acl = a.to_acl(true, me()).unwrap().unwrap();
        assert_eq!(acl.grants.len(), 3);
        assert!(acl.allows_anonymous(Perm::Read) && !acl.allows_anonymous(Perm::WriteAcp));
        let both = AclArgs {
            canned: Some("private"),
            read: Some(r#"id="fakes3""#),
            ..Default::default()
        };
        assert!(both.to_acl(true, me()).is_err());
        let bad = AclArgs {
            read: Some(r#"id="nobody""#),
            ..Default::default()
        };
        assert!(bad.to_acl(true, me()).is_err());
    }

    #[test]
    fn enforced_ownership_disables_acls() {
        let cfg = BucketConfig::default();
        *cfg.ownership.write() = Some(crate::bucket_config::Ownership::Enforced);
        assert!(resolve(&cfg, &args(Some("public-read")), false, me()).is_err());
        assert!(
            resolve(&cfg, &args(Some("bucket-owner-full-control")), false, me())
                .unwrap()
                .is_none()
        );
        assert!(resolve(&cfg, &args(None), false, me()).unwrap().is_none());
    }

    #[test]
    fn new_bucket_with_acls_disabled_accepts_only_owner_acls() {
        let cfg = BucketConfig::default();
        *cfg.ownership.write() = Some(crate::bucket_config::Ownership::Enforced);
        assert!(resolve_for_new_bucket(&cfg, &args(Some("private")), me()).unwrap().is_none());
        let e = resolve_for_new_bucket(&cfg, &args(Some("public-read")), me()).unwrap_err();
        assert_eq!(e.code().as_str(), "InvalidBucketAclWithObjectOwnership");
        let grant = AclArgs {
            read: Some(r#"uri="http://acs.amazonaws.com/groups/global/AllUsers""#),
            ..Default::default()
        };
        assert!(resolve_for_new_bucket(&cfg, &grant, me()).is_err());
    }

    #[test]
    fn block_public_acls_rejects_public_grants() {
        let cfg = BucketConfig::default();
        *cfg.public_access_block.write() = Some(crate::bucket_config::PublicAccessBlock {
            block_public_acls: true,
            ..Default::default()
        });
        assert!(resolve(&cfg, &args(Some("public-read")), true, me()).is_err());
        assert!(resolve(&cfg, &args(Some("private")), true, me()).unwrap().is_none());
    }

    #[test]
    fn dto_round_trip_names_the_grantees() {
        let acl = Acl {
            grants: vec![
                (Grantee::User("fakes3".into()), Perm::FullControl),
                (Grantee::User("cafe".into()), Perm::Write),
                (Grantee::User("0".repeat(64)), Perm::Read),
                (Grantee::Group(Group::AllUsers), Perm::Read),
            ],
        };
        let dto = acl.to_dto(users());
        let name = |i: usize| dto[i].grantee.as_ref().unwrap().display_name.clone();
        assert_eq!(
            (name(0).as_deref(), name(1).as_deref(), name(2), name(3)),
            (Some("fakes3"), Some("alt"), None, None)
        );
        let back = Acl::from_dto(
            AccessControlPolicy {
                grants: Some(dto),
                owner: Some(users().owner(UserId::DEFAULT)),
            },
            users(),
        )
        .unwrap();
        assert_eq!(acl, back);
        // An id that is neither configured nor 64 hex digits is refused.
        let bad = AccessControlPolicy {
            grants: Some(vec![Grant {
                grantee: Some(DtoGrantee {
                    id: Some("nobody".into()),
                    type_: Type::from_static(Type::CANONICAL_USER),
                    display_name: None,
                    email_address: None,
                    uri: None,
                }),
                permission: Some(Permission::from_static(Permission::READ)),
            }]),
            owner: None,
        };
        assert!(Acl::from_dto(bad, users()).is_err());
    }
}
