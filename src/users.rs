//! Identities: the accounts requests act as.
//!
//! fakes3 has no IAM, so an identity is an account: S3's ACLs and ownership are account-level
//! anyway. The table is built at startup and never changes, which lets everything else refer to an
//! identity by its index ([`UserId`], two bytes) instead of a string: buckets, object versions and
//! uploads store one, and a listing renders an owner by borrowing from the table. See
//! `docs/multi-user.md` for the model.
//!
//! The first identity is the configured default credential (`Config::access_key`), the owner of
//! everything before multi-user support existed. Requests whose access key matches no identity
//! (possible only without `--auth`) act as it too.

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use s3s::auth::SimpleAuth;
use s3s::dto::Owner;

/// Name, canonical id and (as `000000000000`) account id of the default identity.
pub const DEFAULT_NAME: &str = "fakes3";
pub const DEFAULT_ACCOUNT_ID: &str = "000000000000";

/// Index of an identity in [`Users`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct UserId(u16);

impl UserId {
    /// The first identity: the configured credential, the owner when nobody else is known.
    pub const DEFAULT: Self = Self(0);

    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// One identity as configured: `--user name:access-key:secret[:account-id[:canonical-id]]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserConfig {
    /// DisplayName in owners and grantees, and the user name in `arn:aws:iam::<account>:user/<name>`.
    pub name: String,
    pub access_key: String,
    pub secret_key: String,
    /// 12 digits. Generated (`000000000001`, `000000000002`, ...) when absent.
    pub account_id: Option<String>,
    /// What ACLs call the account. Generated from the account id when absent.
    pub canonical_id: Option<String>,
}

impl FromStr for UserConfig {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        let parts: Vec<&str> = s.split(':').collect();
        if !(3..=5).contains(&parts.len()) {
            return Err(format!(
                "expected name:access-key:secret[:account-id[:canonical-id]], got `{}` ({} fields)",
                mask(s),
                parts.len()
            ));
        }
        let opt = |i: usize| parts.get(i).filter(|p| !p.is_empty()).map(|p| (*p).to_owned());
        Ok(Self {
            name: parts[0].to_owned(),
            access_key: parts[1].to_owned(),
            secret_key: parts[2].to_owned(),
            account_id: opt(3),
            canonical_id: opt(4),
        })
    }
}

/// The spec with the secret hidden, for error messages.
fn mask(s: &str) -> String {
    let mut parts: Vec<&str> = s.split(':').collect();
    if let Some(p) = parts.get_mut(2) {
        *p = "***";
    }
    parts.join(":")
}

/// A resolved identity.
pub struct User {
    /// DisplayName.
    pub name: String,
    pub access_key: String,
    secret_key: String,
    pub account_id: String,
    pub canonical_id: String,
    /// `arn:aws:iam::<account>:root`.
    pub arn: String,
    /// The `<Owner>` element a listing writes for this identity.
    owner_xml: String,
}

impl User {
    fn new(name: String, access_key: String, secret_key: String, account_id: String, canonical_id: String) -> Self {
        let owner_xml = format!(
            "<Owner><DisplayName>{}</DisplayName><ID>{}</ID></Owner>",
            escape(&name),
            escape(&canonical_id)
        );
        Self {
            arn: format!("arn:aws:iam::{account_id}:root"),
            name,
            access_key,
            secret_key,
            account_id,
            canonical_id,
            owner_xml,
        }
    }

    pub fn secret_key(&self) -> &str {
        &self.secret_key
    }

    /// Whether `principal` from a bucket policy names this identity: its account (`<account>`,
    /// `arn:aws:iam::<account>:root`), itself as a user of that account, or its canonical id.
    pub fn is_principal(&self, principal: &str) -> bool {
        if principal == self.account_id || principal == self.canonical_id {
            return true;
        }
        let Some(rest) = principal
            .strip_prefix("arn:aws:iam::")
            .and_then(|r| r.strip_prefix(self.account_id.as_str()))
        else {
            return false;
        };
        match rest.strip_prefix(':') {
            Some("root") => true,
            Some(user) => user
                .strip_prefix("user/")
                .is_some_and(|p| p.rsplit('/').next() == Some(self.name.as_str())),
            None => false,
        }
    }
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\'' => out.push_str("&apos;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

/// The identities of one server.
pub struct Users {
    list: Vec<User>,
    /// Only built with more than one identity; a single one never needs a lookup.
    by_key: HashMap<String, UserId>,
}

impl fmt::Debug for Users {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.list.iter().map(|u| &u.name)).finish()
    }
}

impl Default for Users {
    fn default() -> Self {
        Self::single("fakes3", "fakes3secret")
    }
}

/// 64 hexadecimal digits that depend only on `seed`: a canonical id for an account that was not
/// given one. FNV-1a, four lanes with different offsets; stable across Rust versions.
fn derive_canonical(seed: &str) -> String {
    (0..4u64)
        .map(|lane| {
            let mut h = 0xcbf2_9ce4_8422_2325u64 ^ lane.wrapping_mul(0x9e37_79b9_7f4a_7c15);
            for b in seed.bytes() {
                h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
            }
            format!("{h:016x}")
        })
        .collect()
}

fn check_field(what: &str, v: &str) -> Result<(), String> {
    if v.is_empty() {
        return Err(format!("{what} is empty"));
    }
    // Names may have spaces (they are only ever displayed); everything else ends up in headers and policies.
    let space = |c: char| c.is_whitespace() && what != "user name";
    if v.chars().any(|c| space(c) || c.is_control() || matches!(c, '"' | ',')) {
        return Err(format!("{what} `{v}` has a control character, quote, comma or space"));
    }
    Ok(())
}

impl Users {
    /// Just the default credential.
    pub fn single(access_key: &str, secret_key: &str) -> Self {
        Self {
            list: vec![User::new(
                DEFAULT_NAME.into(),
                access_key.into(),
                secret_key.into(),
                DEFAULT_ACCOUNT_ID.into(),
                DEFAULT_NAME.into(),
            )],
            by_key: HashMap::new(),
        }
    }

    /// The default credential followed by `extra`, with generated ids filled in. Fails when two
    /// identities share an access key, account id, canonical id or name.
    pub fn new(access_key: &str, secret_key: &str, extra: &[UserConfig]) -> Result<Self, String> {
        let mut users = Self::single(access_key, secret_key);
        check_field("access key", access_key)?;
        for (n, c) in extra.iter().enumerate() {
            let position = n + 1;
            check_field("user name", &c.name)?;
            check_field("access key", &c.access_key)?;
            check_field("secret key", &c.secret_key)?;
            let account_id = c.account_id.clone().unwrap_or_else(|| format!("{position:012}"));
            if account_id.len() != 12 || !account_id.bytes().all(|b| b.is_ascii_digit()) {
                return Err(format!("user `{}`: account id `{account_id}` is not 12 digits", c.name));
            }
            let canonical_id = match &c.canonical_id {
                Some(id) => {
                    check_field("canonical id", id)?;
                    id.clone()
                }
                None => derive_canonical(&account_id),
            };
            let clash = |what: &str, same: &dyn Fn(&User) -> bool| match users.list.iter().find(|u| same(u)) {
                Some(u) => Err(format!("user `{}` has the same {what} as `{}`", c.name, u.name)),
                None => Ok(()),
            };
            clash("access key", &|u| u.access_key == c.access_key)?;
            clash("account id", &|u| u.account_id == account_id)?;
            clash("canonical id", &|u| u.canonical_id == canonical_id)?;
            clash("name", &|u| u.name == c.name)?;
            if users.list.len() >= usize::from(u16::MAX) {
                return Err("too many users".into());
            }
            users.list.push(User::new(
                c.name.clone(),
                c.access_key.clone(),
                c.secret_key.clone(),
                account_id,
                canonical_id,
            ));
        }
        if users.list.len() > 1 {
            users.by_key = users
                .list
                .iter()
                .enumerate()
                .map(|(i, u)| (u.access_key.clone(), UserId(i as u16)))
                .collect();
        }
        Ok(users)
    }

    /// Whether anyone besides the default identity exists. When false, every signed request is
    /// the owner and none of the cross-account logic can matter.
    pub fn multi(&self) -> bool {
        self.list.len() > 1
    }

    pub fn get(&self, id: UserId) -> &User {
        &self.list[id.index()]
    }

    pub fn iter(&self) -> impl Iterator<Item = (UserId, &User)> {
        self.list.iter().enumerate().map(|(i, u)| (UserId(i as u16), u))
    }

    /// The identity of a signature that named `access_key`: the configured one, else the default.
    pub fn resolve(&self, access_key: Option<&str>) -> UserId {
        match access_key {
            Some(k) if self.multi() => self.by_key.get(k).copied().unwrap_or(UserId::DEFAULT),
            _ => UserId::DEFAULT,
        }
    }

    pub fn by_canonical(&self, canonical_id: &str) -> Option<(UserId, &User)> {
        self.iter().find(|(_, u)| u.canonical_id == canonical_id)
    }

    pub fn by_account(&self, account_id: &str) -> Option<(UserId, &User)> {
        self.iter().find(|(_, u)| u.account_id == account_id)
    }

    /// An `Owner` for a response DTO.
    pub fn owner(&self, id: UserId) -> Owner {
        let u = self.get(id);
        Owner {
            display_name: Some(u.name.clone()),
            id: Some(u.canonical_id.clone()),
        }
    }

    /// The `<Owner>` element, for code that writes list XML directly.
    pub fn owner_xml(&self, id: UserId) -> &str {
        &self.get(id).owner_xml
    }

    /// What s3s verifies signatures against with `--auth`: every identity's own secret.
    pub fn auth(&self) -> SimpleAuth {
        let mut auth = SimpleAuth::new();
        for u in &self.list {
            auth.register(u.access_key.clone(), u.secret_key.clone().into());
        }
        auth
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(s: &str) -> UserConfig {
        s.parse().unwrap()
    }

    #[test]
    fn parses_the_spec() {
        let u = spec("alt:AK:SK");
        assert_eq!((u.name.as_str(), u.account_id, u.canonical_id), ("alt", None, None));
        let u = spec("alt:AK:SK:111122223333:abc");
        assert_eq!(
            (u.account_id.as_deref(), u.canonical_id.as_deref()),
            (Some("111122223333"), Some("abc"))
        );
        // Empty optional fields are the same as absent ones.
        assert_eq!(spec("alt:AK:SK::abc").account_id, None);
        assert!("alt:AK".parse::<UserConfig>().is_err());
        let e = "a:b:hunter2:d:e:f".parse::<UserConfig>().unwrap_err();
        assert!(!e.contains("hunter2"), "{e}");
    }

    #[test]
    fn generates_stable_ids() {
        let a = Users::new("fakes3", "s", &[spec("alt:AK:SK"), spec("tenant:AK2:SK2")]).unwrap();
        let b = Users::new("fakes3", "s", &[spec("alt:AK:SK"), spec("tenant:AK2:SK2")]).unwrap();
        let alt = a.get(UserId(1));
        assert_eq!(alt.account_id, "000000000001");
        assert_eq!(a.get(UserId(2)).account_id, "000000000002");
        assert_eq!(alt.canonical_id.len(), 64);
        assert!(alt.canonical_id.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(alt.canonical_id, b.get(UserId(1)).canonical_id);
        assert_ne!(alt.canonical_id, a.get(UserId(2)).canonical_id);
        assert_eq!(a.get(UserId::DEFAULT).canonical_id, "fakes3");
    }

    #[test]
    fn rejects_clashes_and_bad_fields() {
        let e = |extra: &[&str]| {
            let extra: Vec<_> = extra.iter().map(|s| spec(s)).collect();
            Users::new("fakes3", "s", &extra).err()
        };
        assert!(e(&["alt:AK:SK", "alt:AK2:SK"]).unwrap().contains("name"));
        assert!(e(&["a:fakes3:SK"]).unwrap().contains("access key"));
        assert!(e(&["a:AK:SK:000000000000"]).unwrap().contains("account id"));
        assert!(e(&["a:AK:SK:12"]).unwrap().contains("12 digits"));
        assert!(e(&["a:AK:SK:111111111111:fakes3"]).unwrap().contains("canonical id"));
        assert!(e(&["a b:AK:SK"]).is_none());
        assert!(e(&["a:A K:SK"]).is_some());
        assert!(e(&["a\"b:AK:SK"]).is_some());
        assert!(e(&["a:AK:SK:111111111111:xyz", "b:AK2:SK:222222222222:xyz"]).is_some());
        assert!(e(&["a:AK:SK:111111111111:xyz"]).is_none());
    }

    #[test]
    fn resolves_access_keys() {
        let single = Users::default();
        assert_eq!(single.resolve(Some("whatever")), UserId::DEFAULT);
        let users = Users::new("fakes3", "s", &[spec("alt:AK:SK")]).unwrap();
        assert_eq!(users.resolve(Some("AK")), UserId(1));
        assert_eq!(users.resolve(Some("fakes3")), UserId::DEFAULT);
        assert_eq!(users.resolve(Some("unknown")), UserId::DEFAULT);
        assert_eq!(users.resolve(None), UserId::DEFAULT);
    }

    #[test]
    fn principals() {
        let users = Users::new("fakes3", "s", &[spec("alice:AK:SK:111122223333:cafe")]).unwrap();
        let alice = users.get(UserId(1));
        for p in [
            "111122223333",
            "cafe",
            "arn:aws:iam::111122223333:root",
            "arn:aws:iam::111122223333:user/alice",
            "arn:aws:iam::111122223333:user/dev/alice",
        ] {
            assert!(alice.is_principal(p), "{p}");
        }
        for p in [
            "000000000000",
            "arn:aws:iam::111122223333:user/bob",
            "arn:aws:iam::111122223333:role/alice",
            "arn:aws:iam::1111222233334:root",
            "arn:aws:iam::111122223333",
        ] {
            assert!(!alice.is_principal(p), "{p}");
        }
        assert!(users.get(UserId::DEFAULT).is_principal("fakes3"));
    }

    #[test]
    fn owner_xml_matches_the_dto_and_escapes() {
        let users = Users::new("fakes3", "s", &[spec("a&b:AK:SK")]).unwrap();
        assert_eq!(
            users.owner_xml(UserId::DEFAULT),
            "<Owner><DisplayName>fakes3</DisplayName><ID>fakes3</ID></Owner>"
        );
        assert!(users.owner_xml(UserId(1)).contains("<DisplayName>a&amp;b</DisplayName>"));
    }
}
