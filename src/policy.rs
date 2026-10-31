//! Bucket policies: validation on PUT and evaluation per request.
//!
//! Documents are parsed with `s3s-policy`'s model (it has no evaluator, so the matching lives
//! here). A request is evaluated against one bucket policy as AWS does within an account:
//! an explicit Deny always wins, an Allow only matters for requests that would otherwise have no
//! permission (anonymous ones, since the single signed-in identity is the account root).

use std::net::IpAddr;
use std::time::{SystemTime, UNIX_EPOCH};

use http::HeaderMap;
use s3s::dto::{Timestamp, TimestampFormat};
use s3s::{S3Error, S3ErrorCode, S3Result};
use s3s_policy::model::{self as m, ActionRule, Effect, Principal, PrincipalRule, ResourceRule};

use crate::bucket_config::custom_error;

/// The account every signed request belongs to.
pub const ACCOUNT_ID: &str = "000000000000";
const MAX_POLICY_BYTES: usize = 20 * 1024;

pub struct Policy {
    pub json: String,
    doc: m::Policy,
    /// Whether some Allow statement grants access to anyone (what `GetBucketPolicyStatus` reports).
    pub public: bool,
    /// Whether a condition refers to the client address, so the server must record peer addresses.
    pub uses_source_ip: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Verdict {
    Deny,
    Allow,
    NoMatch,
}

/// What a policy condition can ask about the request.
pub struct Context<'a> {
    pub headers: &'a HeaderMap,
    pub query: &'a str,
    pub now: SystemTime,
    pub signed: bool,
    /// The signature was in the query string rather than a header. Only meaningful when `signed`.
    pub presigned: bool,
    pub peer: Option<IpAddr>,
}

fn malformed(msg: &'static str) -> S3Error {
    S3Error::with_message(S3ErrorCode::MalformedPolicy, msg)
}

impl Policy {
    pub fn parse(bucket: &str, json: &str) -> S3Result<Self> {
        if json.len() > MAX_POLICY_BYTES {
            return Err(custom_error(
                "PolicyTooLarge",
                http::StatusCode::BAD_REQUEST,
                "Policy exceeds the maximum allowed document size.",
            ));
        }
        if !json.trim_start().starts_with('{') {
            return Err(malformed("Policies must be valid JSON and the first byte must be '{'"));
        }
        let doc: m::Policy = serde_json::from_str(json).map_err(|_| malformed("Syntax errors in policy."))?;
        validate(bucket, &doc)?;
        let mut p = Self {
            json: json.to_owned(),
            doc,
            public: false,
            uses_source_ip: false,
        };
        p.public = p.doc.statement.as_slice().iter().any(statement_is_public);
        p.uses_source_ip = p.doc.statement.as_slice().iter().any(|s| {
            s.condition
                .iter()
                .flat_map(|c| c.0.values())
                .any(|kv| kv.0.keys().any(|k| k.eq_ignore_ascii_case("aws:SourceIp")))
        });
        Ok(p)
    }

    /// Evaluates the policy for one `(action, resource)` pair.
    pub fn evaluate(&self, action: &str, resource: &str, ctx: &Context<'_>) -> Verdict {
        let mut allowed = false;
        for st in self.doc.statement.as_slice() {
            if !statement_applies(st, action, resource, ctx) {
                continue;
            }
            match st.effect {
                Effect::Deny => return Verdict::Deny,
                Effect::Allow => allowed = true,
            }
        }
        if allowed { Verdict::Allow } else { Verdict::NoMatch }
    }
}

fn statement_applies(st: &m::Statement, action: &str, resource: &str, ctx: &Context<'_>) -> bool {
    principal_matches(st.principal.as_ref(), ctx.signed)
        && match &st.action {
            ActionRule::Action(a) => list_matches(a.as_slice(), action, true),
            ActionRule::NotAction(a) => !list_matches(a.as_slice(), action, true),
        }
        && match &st.resource {
            ResourceRule::Resource(r) => list_matches(r.as_slice(), resource, false),
            ResourceRule::NotResource(r) => !list_matches(r.as_slice(), resource, false),
        }
        && st.condition.as_ref().is_none_or(|c| conditions_hold(c, ctx))
}

/// `None` is the wildcard (`"*"`).
fn list_matches(patterns: Option<&[String]>, text: &str, case_insensitive: bool) -> bool {
    patterns.is_none_or(|ps| ps.iter().any(|p| wildcard(p, text, case_insensitive)))
}

/// `*` matches any run of characters and `?` any one; everything else matches itself.
pub fn wildcard(pattern: &str, text: &str, case_insensitive: bool) -> bool {
    let (p, t) = (pattern.as_bytes(), text.as_bytes());
    let eq = |a: u8, b: u8| if case_insensitive { a.eq_ignore_ascii_case(&b) } else { a == b };
    let (mut pi, mut ti) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == b'?' || (p[pi] != b'*' && eq(p[pi], t[ti]))) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == b'*' {
            star = Some((pi, ti));
            pi += 1;
        } else if let Some((sp, st)) = star {
            pi = sp + 1;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|b| *b == b'*')
}

fn is_account_principal(s: &str) -> bool {
    s == ACCOUNT_ID
        || s.strip_prefix("arn:aws:iam::")
            .and_then(|r| r.strip_prefix(ACCOUNT_ID))
            .is_some_and(|r| r.starts_with(':'))
}

fn principal_matches(rule: Option<&PrincipalRule>, signed: bool) -> bool {
    let (p, negate) = match rule {
        Some(PrincipalRule::Principal(p)) => (p, false),
        Some(PrincipalRule::NotPrincipal(p)) => (p, true),
        None => return false,
    };
    let hit = match p {
        Principal::Wildcard => true,
        Principal::Map(map) => map.iter().any(|(kind, values)| {
            (kind == "AWS" || kind == "CanonicalUser")
                && values
                    .as_slice()
                    .iter()
                    .any(|v| v == "*" || (signed && (is_account_principal(v) || v == crate::acl::OWNER_ID)))
        }),
    };
    hit != negate
}

// ---- conditions ------------------------------------------------------------------------------

const OPERATORS: &[&str] = &[
    "stringequals",
    "stringnotequals",
    "stringequalsignorecase",
    "stringnotequalsignorecase",
    "stringlike",
    "stringnotlike",
    "numericequals",
    "numericnotequals",
    "numericlessthan",
    "numericlessthanequals",
    "numericgreaterthan",
    "numericgreaterthanequals",
    "dateequals",
    "datenotequals",
    "datelessthan",
    "datelessthanequals",
    "dategreaterthan",
    "dategreaterthanequals",
    "bool",
    "binaryequals",
    "ipaddress",
    "notipaddress",
    "arnequals",
    "arnlike",
    "arnnotequals",
    "arnnotlike",
    "null",
];

struct Op {
    base: String,
    /// The comparison with `Not` removed (`stringnotequals` -> `stringequals`).
    positive: String,
    negated: bool,
    if_exists: bool,
    for_all: bool,
}

fn parse_operator(op: &str) -> Option<Op> {
    let lower = op.to_ascii_lowercase();
    let (for_all, rest) = if let Some(r) = lower.strip_prefix("forallvalues:") {
        (true, r)
    } else if let Some(r) = lower.strip_prefix("foranyvalue:") {
        (false, r)
    } else {
        (false, lower.as_str())
    };
    let (rest, if_exists) = match rest.strip_suffix("ifexists") {
        Some(r) => (r, true),
        None => (rest, false),
    };
    if !OPERATORS.contains(&rest) {
        return None;
    }
    let negated = rest.contains("not");
    let positive = if negated { rest.replacen("not", "", 1) } else { rest.to_owned() };
    Some(Op {
        base: rest.to_owned(),
        positive,
        negated,
        if_exists,
        for_all,
    })
}

fn conditions_hold(cond: &m::ConditionRule, ctx: &Context<'_>) -> bool {
    cond.0.iter().all(|(op, kv)| {
        let Some(op) = parse_operator(op) else { return false };
        kv.0.iter().all(|(key, want)| condition_holds(&op, key, want.as_slice(), ctx))
    })
}

fn condition_holds(op: &Op, key: &str, want: &[String], ctx: &Context<'_>) -> bool {
    let have = ctx.values(&key.to_ascii_lowercase());
    if op.base == "null" {
        return have.is_empty() == want.first().is_some_and(|w| w.eq_ignore_ascii_case("true"));
    }
    if have.is_empty() {
        return op.if_exists || op.negated || op.for_all;
    }
    let want: Vec<String> = want.iter().map(|w| ctx.substitute(w)).collect();
    let any_match = |h: &String| want.iter().any(|w| compare(&op.positive, h, w));
    let hit = if op.for_all {
        have.iter().all(any_match)
    } else {
        have.iter().any(any_match)
    };
    // A negated operator holds when none of the policy values match.
    if op.negated && !op.for_all {
        !have.iter().any(any_match)
    } else {
        hit != op.negated
    }
}

fn compare(op: &str, have: &str, want: &str) -> bool {
    match op {
        "stringequals" | "binaryequals" => have == want,
        "stringlike" | "arnequals" | "arnlike" => wildcard(want, have, false),
        "stringequalsignorecase" => have.eq_ignore_ascii_case(want),
        "bool" => have.eq_ignore_ascii_case(want),
        "ipaddress" => ip_matches(have, want),
        o if o.starts_with("numeric") => match (have.parse::<f64>(), want.parse::<f64>()) {
            (Ok(h), Ok(w)) => ordered(&o[7..], h.partial_cmp(&w)),
            _ => false,
        },
        o if o.starts_with("date") => match (parse_date(have), parse_date(want)) {
            (Some(h), Some(w)) => ordered(&o[4..], Some(h.cmp(&w))),
            _ => false,
        },
        _ => false,
    }
}

fn ordered(op: &str, ord: Option<std::cmp::Ordering>) -> bool {
    use std::cmp::Ordering::*;
    matches!(
        (op, ord),
        ("equals", Some(Equal))
            | ("lessthan", Some(Less))
            | ("lessthanequals", Some(Less | Equal))
            | ("greaterthan", Some(Greater))
            | ("greaterthanequals", Some(Greater | Equal))
    )
}

fn parse_date(s: &str) -> Option<i64> {
    if let Ok(secs) = s.parse::<i64>() {
        return Some(secs);
    }
    let ts = Timestamp::parse(TimestampFormat::DateTime, s).ok()?;
    Some(time::OffsetDateTime::from(ts).unix_timestamp())
}

fn ip_matches(have: &str, cidr: &str) -> bool {
    let Ok(ip) = have.parse::<IpAddr>() else { return false };
    let (net, len) = match cidr.split_once('/') {
        Some((n, l)) => (n, l.parse::<u32>().ok()),
        None => (cidr, None),
    };
    let Ok(net) = net.parse::<IpAddr>() else { return false };
    match (ip, net) {
        (IpAddr::V4(a), IpAddr::V4(b)) => {
            let len = len.unwrap_or(32).min(32);
            let mask = if len == 0 { 0 } else { u32::MAX << (32 - len) };
            u32::from(a) & mask == u32::from(b) & mask
        }
        (IpAddr::V6(a), IpAddr::V6(b)) => {
            let len = len.unwrap_or(128).min(128);
            let mask = if len == 0 { 0 } else { u128::MAX << (128 - len) };
            u128::from(a) & mask == u128::from(b) & mask
        }
        _ => false,
    }
}

impl Context<'_> {
    fn header(&self, name: &str) -> Vec<String> {
        self.headers
            .get_all(name)
            .iter()
            .filter_map(|v| v.to_str().ok().map(str::to_owned))
            .collect()
    }

    fn query_param(&self, name: &str) -> Vec<String> {
        form_urlencoded::parse(self.query.as_bytes())
            .filter(|(k, _)| k == name)
            .map(|(_, v)| v.into_owned())
            .collect()
    }

    /// Values of a (lowercased) condition key; empty when the request has none.
    fn values(&self, key: &str) -> Vec<String> {
        let epoch = self.now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        match key {
            "aws:securetransport" => {
                let tls = self.headers.get("x-forwarded-proto").is_some_and(|v| v == "https");
                vec![tls.to_string()]
            }
            "aws:currenttime" => {
                let mut buf = Vec::new();
                match Timestamp::from(self.now).format(TimestampFormat::DateTime, &mut buf) {
                    Ok(()) => vec![String::from_utf8_lossy(&buf).into_owned()],
                    Err(_) => Vec::new(),
                }
            }
            "aws:epochtime" => vec![epoch.to_string()],
            "aws:sourceip" => self.peer.map(|p| p.to_string()).into_iter().collect(),
            "aws:useragent" => self.header("user-agent"),
            "aws:referer" => self.header("referer"),
            "aws:principaltype" => vec![if self.signed { "Account" } else { "Anonymous" }.to_owned()],
            "aws:principalarn" if self.signed => vec![format!("arn:aws:iam::{ACCOUNT_ID}:root")],
            "aws:userid" | "aws:principalaccount" if self.signed => vec![ACCOUNT_ID.to_owned()],
            "s3:prefix" => self.query_param("prefix"),
            "s3:delimiter" => self.query_param("delimiter"),
            "s3:max-keys" => self.query_param("max-keys"),
            "s3:versionid" => self.query_param("versionId"),
            "s3:authtype" if self.signed => {
                vec![if self.presigned { "REST-QUERY-STRING" } else { "REST-HEADER" }.to_owned()]
            }
            "s3:requestobjecttagkeys" => self.request_tags().into_iter().map(|(k, _)| k).collect(),
            k if k.starts_with("s3:requestobjecttag/") => {
                let want = &k["s3:requestobjecttag/".len()..];
                self.request_tags()
                    .into_iter()
                    .filter(|(k, _)| k.eq_ignore_ascii_case(want))
                    .map(|(_, v)| v)
                    .collect()
            }
            k if k.starts_with("s3:") && !k.contains('/') => self.header(&k[3..]),
            _ => Vec::new(),
        }
    }

    fn request_tags(&self) -> Vec<(String, String)> {
        self.header("x-amz-tagging")
            .iter()
            .flat_map(|h| {
                form_urlencoded::parse(h.as_bytes())
                    .map(|(k, v)| (k.into_owned(), v.into_owned()))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// Expands `${aws:...}` policy variables in a condition value.
    fn substitute(&self, s: &str) -> String {
        if !s.contains("${") {
            return s.to_owned();
        }
        let mut out = String::with_capacity(s.len());
        let mut rest = s;
        while let Some(i) = rest.find("${") {
            out.push_str(&rest[..i]);
            let Some(end) = rest[i..].find('}') else {
                rest = &rest[i..];
                break;
            };
            let var = &rest[i + 2..i + end];
            match var {
                "*" | "?" | "$" => out.push_str(var),
                _ => out.push_str(self.values(&var.to_ascii_lowercase()).first().map_or("", String::as_str)),
            }
            rest = &rest[i + end + 1..];
        }
        out.push_str(rest);
        out
    }
}

// ---- validation and public-ness -------------------------------------------------------------

#[derive(PartialEq)]
enum Scope {
    Bucket,
    Object,
    Any,
}

fn action_scope(action: &str) -> Scope {
    let a = action.to_ascii_lowercase();
    let a = a.strip_prefix("s3:").unwrap_or(&a);
    if a.contains(['*', '?']) {
        return Scope::Any;
    }
    const BUCKET_PREFIXES: [&str; 5] = ["getbucket", "putbucket", "deletebucket", "listbucket", "createbucket"];
    if BUCKET_PREFIXES.iter().any(|p| a.starts_with(p)) || a.ends_with("configuration") {
        return Scope::Bucket;
    }
    let object_actions = [
        "abortmultipartupload",
        "listmultipartuploadparts",
        "restoreobject",
        "bypassgovernanceretention",
    ];
    if object_actions.contains(&a) || a.contains("object") || a.starts_with("replicate") {
        return Scope::Object;
    }
    Scope::Any
}

fn resource_scope(resource: &str) -> Scope {
    match resource.strip_prefix("arn:aws:s3:::") {
        Some(r) if r.contains('/') => Scope::Object,
        Some(r) if r.contains(['*', '?']) => Scope::Any,
        Some(_) => Scope::Bucket,
        None => Scope::Any,
    }
}

fn validate(bucket: &str, doc: &m::Policy) -> S3Result<()> {
    for st in doc.statement.as_slice() {
        if st.principal.is_none() {
            return Err(malformed("Missing required field Principal"));
        }
        if let Some(PrincipalRule::Principal(Principal::Map(map)) | PrincipalRule::NotPrincipal(Principal::Map(map))) = &st.principal
            && map
                .keys()
                .any(|k| !["AWS", "Service", "Federated", "CanonicalUser"].contains(&k.as_str()))
        {
            return Err(malformed("Invalid principal in policy"));
        }
        let (actions, not_action) = match &st.action {
            ActionRule::Action(a) => (a.as_slice(), false),
            ActionRule::NotAction(a) => (a.as_slice(), true),
        };
        for a in actions.unwrap_or_default() {
            if !a.starts_with("s3:") && !a.starts_with("s3express:") && a != "*" {
                return Err(malformed("Policy has invalid action"));
            }
        }
        let resources = match &st.resource {
            ResourceRule::Resource(r) => r.as_slice(),
            ResourceRule::NotResource(_) => None,
        };
        let mut resource_scopes = Vec::new();
        for r in resources.unwrap_or_default() {
            let Some(rest) = r.strip_prefix("arn:aws:s3:::") else {
                return Err(malformed("Policy has invalid resource"));
            };
            let bucket_part = rest.split('/').next().unwrap_or_default();
            if !wildcard(bucket_part, bucket, false) {
                return Err(malformed("Policy has invalid resource"));
            }
            resource_scopes.push(resource_scope(r));
        }
        if let (false, Some(_)) = (not_action, resources) {
            for a in actions.unwrap_or_default() {
                let s = action_scope(a);
                if s != Scope::Any && !resource_scopes.iter().any(|r| *r == Scope::Any || *r == s) {
                    return Err(malformed("Action does not apply to any resource(s) in statement"));
                }
            }
        }
        if let Some(c) = &st.condition {
            for op in c.0.keys() {
                if parse_operator(op).is_none() {
                    return Err(malformed("Invalid Condition operator"));
                }
            }
        }
    }
    Ok(())
}

/// Condition keys that pin a statement to particular callers or networks.
const RESTRICTING_KEYS: &[&str] = &[
    "aws:principalorgid",
    "aws:principalorgpaths",
    "aws:principalaccount",
    "aws:principalarn",
    "aws:sourcevpc",
    "aws:sourcevpce",
    "aws:sourcearn",
    "aws:sourceaccount",
    "aws:sourceowner",
    "aws:userid",
    "aws:username",
    "s3:dataaccesspointaccount",
    "s3:dataaccesspointarn",
];

/// An Allow for everyone that no condition narrows down.
fn statement_is_public(st: &m::Statement) -> bool {
    if st.effect != Effect::Allow {
        return false;
    }
    let everyone = match &st.principal {
        Some(PrincipalRule::Principal(Principal::Wildcard)) => true,
        Some(PrincipalRule::Principal(Principal::Map(map))) => map
            .iter()
            .any(|(k, v)| (k == "AWS" || k == "CanonicalUser") && v.as_slice().iter().any(|s| s == "*")),
        _ => false,
    };
    everyone
        && st.condition.iter().flat_map(|c| c.0.iter()).all(|(op, kv)| {
            let positive = parse_operator(op).is_some_and(|o| !o.negated);
            !kv.0.iter().any(|(key, vals)| {
                let key = key.to_ascii_lowercase();
                let wide_ip = key == "aws:sourceip" && vals.as_slice().iter().any(|v| v == "0.0.0.0/0" || v == "::/0");
                positive && !wide_ip && RESTRICTING_KEYS.iter().chain(&["aws:sourceip"]).any(|k| *k == key)
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(signed: bool) -> Context<'static> {
        let headers: &'static HeaderMap = Box::leak(Box::default());
        Context {
            headers,
            query: "prefix=logs%2F&max-keys=5",
            now: UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000),
            signed,
            presigned: false,
            peer: Some("10.1.2.3".parse().unwrap()),
        }
    }

    fn parse(json: &str) -> S3Result<Policy> {
        Policy::parse("b", json)
    }

    #[test]
    fn wildcards() {
        assert!(wildcard("s3:Get*", "s3:getobject", true));
        assert!(!wildcard("s3:Get*", "s3:getobject", false));
        assert!(wildcard("a?c*", "abcdef", false));
        assert!(wildcard("*", "", false));
        assert!(!wildcard("a*b", "acd", false));
        assert!(wildcard("*/x/*", "a/x/b", false));
    }

    #[test]
    fn allow_and_deny() {
        let p = parse(
            r#"{"Version":"2012-10-17","Statement":[
              {"Effect":"Allow","Principal":"*","Action":"s3:GetObject","Resource":"arn:aws:s3:::b/*"},
              {"Effect":"Deny","Principal":{"AWS":"*"},"Action":"s3:GetObject","Resource":"arn:aws:s3:::b/secret/*"}]}"#,
        )
        .unwrap();
        assert!(p.public);
        let c = ctx(false);
        assert_eq!(p.evaluate("s3:GetObject", "arn:aws:s3:::b/x", &c), Verdict::Allow);
        assert_eq!(p.evaluate("s3:GetObject", "arn:aws:s3:::b/secret/x", &c), Verdict::Deny);
        assert_eq!(p.evaluate("s3:PutObject", "arn:aws:s3:::b/x", &c), Verdict::NoMatch);
    }

    #[test]
    fn principals() {
        let p = parse(
            r#"{"Statement":{"Effect":"Deny","Principal":{"AWS":"arn:aws:iam::000000000000:root"},"Action":"s3:*","Resource":["arn:aws:s3:::b","arn:aws:s3:::b/*"]}}"#,
        )
        .unwrap();
        assert!(!p.public);
        assert_eq!(p.evaluate("s3:ListBucket", "arn:aws:s3:::b", &ctx(true)), Verdict::Deny);
        assert_eq!(p.evaluate("s3:ListBucket", "arn:aws:s3:::b", &ctx(false)), Verdict::NoMatch);
    }

    #[test]
    fn conditions() {
        let p = parse(
            r#"{"Statement":[{"Effect":"Allow","Principal":"*","Action":"s3:ListBucket","Resource":"arn:aws:s3:::b",
              "Condition":{"StringLike":{"s3:prefix":["logs/*"]},"NumericLessThanEquals":{"s3:max-keys":"10"},
                           "IpAddress":{"aws:SourceIp":"10.0.0.0/8"}}}]}"#,
        )
        .unwrap();
        assert!(!p.public && p.uses_source_ip);
        assert_eq!(p.evaluate("s3:ListBucket", "arn:aws:s3:::b", &ctx(false)), Verdict::Allow);
        let mut c = ctx(false);
        c.query = "prefix=other%2F";
        assert_eq!(p.evaluate("s3:ListBucket", "arn:aws:s3:::b", &c), Verdict::NoMatch);
        c.query = "prefix=logs%2F&max-keys=50";
        assert_eq!(p.evaluate("s3:ListBucket", "arn:aws:s3:::b", &c), Verdict::NoMatch);
    }

    #[test]
    fn missing_keys_and_negation() {
        let deny_unencrypted = parse(
            r#"{"Statement":[{"Effect":"Deny","Principal":"*","Action":"s3:PutObject","Resource":"arn:aws:s3:::b/*",
              "Condition":{"Null":{"s3:x-amz-server-side-encryption":"true"}}}]}"#,
        )
        .unwrap();
        assert_eq!(
            deny_unencrypted.evaluate("s3:PutObject", "arn:aws:s3:::b/k", &ctx(true)),
            Verdict::Deny
        );
        let not_equals = parse(
            r#"{"Statement":[{"Effect":"Deny","Principal":"*","Action":"s3:*","Resource":"arn:aws:s3:::b/*",
              "Condition":{"StringNotEquals":{"s3:x-amz-acl":"private"}}}]}"#,
        )
        .unwrap();
        // The key is absent, so a negated comparison holds.
        assert_eq!(not_equals.evaluate("s3:PutObject", "arn:aws:s3:::b/k", &ctx(true)), Verdict::Deny);
    }

    #[test]
    fn date_conditions_use_the_given_time() {
        let p = parse(
            r#"{"Statement":[{"Effect":"Deny","Principal":"*","Action":"s3:*","Resource":"arn:aws:s3:::b/*",
              "Condition":{"DateGreaterThan":{"aws:CurrentTime":"2023-01-01T00:00:00Z"}}}]}"#,
        )
        .unwrap();
        assert_eq!(p.evaluate("s3:GetObject", "arn:aws:s3:::b/k", &ctx(true)), Verdict::Deny);
        let mut early = ctx(true);
        early.now = UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
        assert_eq!(p.evaluate("s3:GetObject", "arn:aws:s3:::b/k", &early), Verdict::NoMatch);
    }

    #[test]
    fn restricted_policies_are_not_public() {
        let p = parse(
            r#"{"Statement":[{"Effect":"Allow","Principal":"*","Action":"s3:GetObject","Resource":"arn:aws:s3:::b/*",
              "Condition":{"StringEquals":{"aws:PrincipalOrgID":"o-123"}}}]}"#,
        )
        .unwrap();
        assert!(!p.public);
    }

    #[test]
    fn validation_errors() {
        let code = |j: &str| parse(j).err().map(|e| e.code().as_str().to_owned());
        let ok = r#"{"Effect":"Allow","Principal":"*","Action":"s3:GetObject","Resource":"arn:aws:s3:::b/*"}"#;
        assert_eq!(code(&format!(r#"{{"Statement":[{ok}]}}"#)), None);
        assert_eq!(code("not json").as_deref(), Some("MalformedPolicy"));
        assert_eq!(
            code(r#"{"Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"arn:aws:s3:::b/*"}]}"#).as_deref(),
            Some("MalformedPolicy")
        );
        assert_eq!(
            code(r#"{"Statement":[{"Effect":"Allow","Principal":"*","Action":"s3:GetObject","Resource":"arn:aws:s3:::other/*"}]}"#)
                .as_deref(),
            Some("MalformedPolicy")
        );
        assert_eq!(
            code(r#"{"Statement":[{"Effect":"Allow","Principal":"*","Action":"ec2:Run","Resource":"arn:aws:s3:::b/*"}]}"#).as_deref(),
            Some("MalformedPolicy")
        );
        // An object action on the bucket ARN applies to nothing.
        assert_eq!(
            code(r#"{"Statement":[{"Effect":"Allow","Principal":"*","Action":"s3:GetObject","Resource":"arn:aws:s3:::b"}]}"#).as_deref(),
            Some("MalformedPolicy")
        );
        assert_eq!(code(r#"{"Statement":[{"Effect":"Allow","Principal":"*","Action":["s3:GetObject","s3:ListBucket"],"Resource":"arn:aws:s3:::b/*"}]}"#).as_deref(), Some("MalformedPolicy"));
    }
}
