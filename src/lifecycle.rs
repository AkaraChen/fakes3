//! Lifecycle configuration: validation, and enforcement against the server clock.
//!
//! S3 evaluates lifecycle rules asynchronously and rounds every expiration time up to midnight
//! UTC, so the set of expired things only changes when the UTC day changes (or the rules do).
//! fakes3 exploits that: a bucket with rules remembers the UTC day (server clock) it last applied
//! them for, and the first request that touches the bucket on a different day runs a sweep before
//! doing its own work. Moving the clock therefore needs no hook: the next request after it sees
//! the world as S3 would have left it. Buckets without rules never look at the clock.
//!
//! The sweep covers expiration of current versions (permanent delete when unversioned, a delete
//! marker when versioned), noncurrent version expiration (with `NewerNoncurrentVersions`),
//! removal of expired object delete markers and aborting stale multipart uploads, and storage
//! class transitions of current and noncurrent versions. A transition only rewrites the object's
//! storage class (its data and `Last-Modified` stay); the waterfall model and the 128 KB default
//! minimum size follow S3, and Object Lock never blocks it.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use s3s::dto::{
    BucketLifecycleConfiguration, ExpirationStatus, LifecycleRule, StorageClass, Timestamp, TimestampFormat,
    TransitionDefaultMinimumObjectSize, TransitionStorageClass,
};
use s3s::{S3Result, s3_error};

use crate::bucket_config::BucketConfig;
use crate::store::{Bucket, Entry, Object, Version, Versioning, new_version_id};
use crate::users::UserId;

const DAY: i64 = 86_400;

pub struct Lifecycle {
    /// The document as submitted, returned verbatim by GET.
    pub config: BucketLifecycleConfiguration,
    pub transition_default_minimum_object_size: Option<TransitionDefaultMinimumObjectSize>,
    rules: Vec<Rule>,
}

struct Rule {
    id: Option<String>,
    prefix: String,
    tags: Vec<(String, String)>,
    larger_than: Option<i64>,
    smaller_than: Option<i64>,
    expiration: Option<Expiration>,
    expired_delete_marker: bool,
    noncurrent: Option<Noncurrent>,
    abort_days: Option<i64>,
    transitions: Vec<(Expiration, &'static str)>,
    noncurrent_transitions: Vec<NoncurrentMove>,
}

struct NoncurrentMove {
    days: i64,
    newer: Option<usize>,
    class: &'static str,
}

/// Objects below this size are not transitioned unless a size filter says otherwise.
const MIN_TRANSITION_SIZE: u64 = 128 * 1024;

/// A storage class's place in the transition waterfall; objects only move to a higher number.
fn rank(class: &str) -> Option<u8> {
    Some(match class {
        StorageClass::STANDARD | StorageClass::REDUCED_REDUNDANCY => 0,
        StorageClass::STANDARD_IA => 1,
        StorageClass::INTELLIGENT_TIERING => 2,
        StorageClass::ONEZONE_IA => 3,
        StorageClass::GLACIER_IR => 4,
        StorageClass::GLACIER => 5,
        StorageClass::DEEP_ARCHIVE => 6,
        _ => return None,
    })
}

/// The class a rule may name as a transition target, as a `'static` name.
fn target(c: &TransitionStorageClass) -> S3Result<&'static str> {
    [
        StorageClass::STANDARD_IA,
        StorageClass::INTELLIGENT_TIERING,
        StorageClass::ONEZONE_IA,
        StorageClass::GLACIER_IR,
        StorageClass::GLACIER,
        StorageClass::DEEP_ARCHIVE,
    ]
    .into_iter()
    .find(|n| *n == c.as_str())
    .ok_or_else(malformed)
}

#[derive(Clone, Copy)]
enum Expiration {
    Days(i64),
    /// Seconds since the epoch.
    Date(i64),
}

struct Noncurrent {
    days: i64,
    newer: Option<usize>,
}

fn malformed() -> s3s::S3Error {
    s3_error!(
        MalformedXML,
        "The XML you provided was not well-formed or did not validate against our published schema"
    )
}

pub(crate) fn secs(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    }
}

/// `created + days`, rounded up to the next midnight UTC, in seconds since the epoch.
pub(crate) fn due(created: SystemTime, days: i64) -> i64 {
    (secs(created) + days * DAY + DAY - 1).div_euclid(DAY) * DAY
}

/// `Sun, 04 Jan 1970 00:00:00 GMT`.
pub(crate) fn http_date(at: i64) -> Option<String> {
    let mut buf = Vec::new();
    Timestamp::from(UNIX_EPOCH + Duration::from_secs(at.max(0) as u64))
        .format(TimestampFormat::HttpDate, &mut buf)
        .ok()?;
    String::from_utf8(buf).ok()
}

impl Lifecycle {
    pub fn new(mut config: BucketLifecycleConfiguration) -> S3Result<Self> {
        if config.rules.is_empty() || config.rules.len() > 1000 {
            return Err(malformed());
        }
        // A rule submitted without an ID gets a generated one, which GET returns.
        for r in config.rules.iter_mut().filter(|r| r.id.is_none()) {
            r.id = Some(uuid::Uuid::new_v4().simple().to_string());
        }
        let mut ids = HashSet::new();
        let mut rules = Vec::with_capacity(config.rules.len());
        for r in &config.rules {
            if let Some(id) = &r.id {
                if id.len() > 255 {
                    return Err(s3_error!(InvalidArgument, "ID length should not exceed allowed limit of 255"));
                }
                if !ids.insert(id.as_str()) {
                    return Err(s3_error!(
                        InvalidArgument,
                        "Rule ID must be unique. Found same ID for more than one rule"
                    ));
                }
            }
            if !matches!(r.status.as_str(), ExpirationStatus::ENABLED | ExpirationStatus::DISABLED) {
                return Err(malformed());
            }
            rules.push(Rule::new(r)?);
        }
        Ok(Self {
            config,
            transition_default_minimum_object_size: None,
            rules,
        })
    }

    /// `obj` moved to the deepest class a due transition names, if that is further down the
    /// waterfall than where it is.
    fn transition_current(&self, key: &str, obj: &Object, now: i64, expires_at: Option<i64>) -> Option<Object> {
        let due_at = |r: &Rule| {
            r.transitions
                .iter()
                .map(|(w, c)| match *w {
                    Expiration::Days(d) => (due(obj.last_modified, d), *c),
                    Expiration::Date(at) => (at, *c),
                })
                .filter(|(at, _)| *at <= now && expires_at.is_none_or(|e| *at <= e))
                .map(|(at, c)| (c, at))
                .collect::<Vec<_>>()
        };
        let moves = self
            .rules
            .iter()
            .filter(|r| !r.transitions.is_empty() && r.matches(key, Some(obj)))
            .flat_map(|r| due_at(r).into_iter().filter(|(c, _)| r.size_allows(self, obj.data.size, c)));
        moved(obj, moves)
    }

    fn transition_noncurrent(&self, key: &str, obj: &Object, since: SystemTime, newer: usize, now: i64) -> Option<Object> {
        let moves = self
            .rules
            .iter()
            .filter(|r| !r.noncurrent_transitions.is_empty() && r.matches(key, Some(obj)))
            .flat_map(|r| {
                r.noncurrent_transitions
                    .iter()
                    .filter(move |n| due(since, n.days) <= now && n.newer.is_none_or(|keep| newer >= keep))
                    .filter(|n| r.size_allows(self, obj.data.size, n.class))
                    .map(|n| (n.class, due(since, n.days)))
            });
        moved(obj, moves)
    }

    /// The `x-amz-expiration` value for a current object, if an enabled rule will expire it.
    pub fn expiration_header(&self, key: &str, obj: &Object) -> Option<String> {
        let (at, rule) = self
            .rules
            .iter()
            .filter(|r| r.matches(key, Some(obj)))
            .filter_map(|r| r.expiration_due(obj.last_modified).map(|d| (d, r)))
            .min_by_key(|(d, _)| *d)?;
        let date = http_date(at)?;
        Some(match &rule.id {
            Some(id) => format!("expiry-date=\"{date}\", rule-id=\"{id}\""),
            None => format!("expiry-date=\"{date}\""),
        })
    }
}

impl Rule {
    fn new(r: &LifecycleRule) -> S3Result<Self> {
        let (prefix, tags, larger_than, smaller_than) = match (&r.filter, &r.prefix) {
            (Some(_), Some(_)) => return Err(malformed()),
            (None, None) => return Err(malformed()),
            (None, Some(p)) => (p.clone(), Vec::new(), None, None),
            (Some(f), None) => {
                let singles = [
                    f.prefix.is_some(),
                    f.tag.is_some(),
                    f.object_size_greater_than.is_some(),
                    f.object_size_less_than.is_some(),
                    f.and.is_some(),
                ];
                if singles.iter().filter(|s| **s).count() > 1 {
                    return Err(s3_error!(
                        InvalidRequest,
                        "Filter element can only have one of Prefix, Tag, or And specified."
                    ));
                }
                if let Some(and) = &f.and {
                    let n = and.prefix.is_some() as usize
                        + and.tags.as_ref().map_or(0, Vec::len)
                        + and.object_size_greater_than.is_some() as usize
                        + and.object_size_less_than.is_some() as usize;
                    if n < 2 {
                        return Err(malformed());
                    }
                    let tags = and
                        .tags
                        .iter()
                        .flatten()
                        .map(|t| (t.key.clone().unwrap_or_default(), t.value.clone().unwrap_or_default()));
                    (
                        and.prefix.clone().unwrap_or_default(),
                        tags.collect(),
                        and.object_size_greater_than,
                        and.object_size_less_than,
                    )
                } else {
                    let tags = f
                        .tag
                        .iter()
                        .map(|t| (t.key.clone().unwrap_or_default(), t.value.clone().unwrap_or_default()));
                    (
                        f.prefix.clone().unwrap_or_default(),
                        tags.collect(),
                        f.object_size_greater_than,
                        f.object_size_less_than,
                    )
                }
            }
        };

        let has_action = r.expiration.is_some()
            || r.noncurrent_version_expiration.is_some()
            || r.abort_incomplete_multipart_upload.is_some()
            || r.transitions.as_ref().is_some_and(|t| !t.is_empty())
            || r.noncurrent_version_transitions.as_ref().is_some_and(|t| !t.is_empty());
        if !has_action {
            return Err(s3_error!(InvalidRequest, "At least one action needs to be specified in a rule"));
        }

        let mut expiration = None;
        let mut expired_delete_marker = false;
        if let Some(e) = &r.expiration {
            let set = e.days.is_some() as u8 + e.date.is_some() as u8 + e.expired_object_delete_marker.is_some() as u8;
            if set != 1 {
                return Err(malformed());
            }
            if let Some(d) = e.days {
                if d <= 0 {
                    return Err(s3_error!(
                        InvalidArgument,
                        "'Days' for Expiration action must be a positive integer"
                    ));
                }
                expiration = Some(Expiration::Days(i64::from(d)));
            }
            if let Some(date) = &e.date {
                let at = time::OffsetDateTime::from(date.clone());
                if at.unix_timestamp() % DAY != 0 || at.nanosecond() != 0 {
                    return Err(s3_error!(InvalidArgument, "'Date' must be at midnight GMT"));
                }
                expiration = Some(Expiration::Date(at.unix_timestamp()));
            }
            // `false` is the same as not saying anything.
            expired_delete_marker = e.expired_object_delete_marker == Some(true);
            if expired_delete_marker && (!tags.is_empty() || larger_than.is_some() || smaller_than.is_some()) {
                return Err(s3_error!(
                    InvalidRequest,
                    "Delete marker expiration cannot be specified with Tags or ObjectSize filters"
                ));
            }
        }

        let noncurrent = match &r.noncurrent_version_expiration {
            None => None,
            Some(n) => {
                let days = n.noncurrent_days.ok_or_else(malformed)?;
                if days <= 0 {
                    return Err(s3_error!(
                        InvalidArgument,
                        "'NoncurrentDays' in the NoncurrentVersionExpiration action must be a positive integer"
                    ));
                }
                if n.newer_noncurrent_versions.is_some_and(|v| v <= 0) {
                    return Err(s3_error!(InvalidArgument, "'NewerNoncurrentVersions' must be a positive integer"));
                }
                Some(Noncurrent {
                    days: i64::from(days),
                    newer: n.newer_noncurrent_versions.map(|v| v as usize),
                })
            }
        };

        let abort_days = match &r.abort_incomplete_multipart_upload {
            None => None,
            Some(a) => {
                let days = a.days_after_initiation.ok_or_else(malformed)?;
                if days <= 0 {
                    return Err(s3_error!(InvalidArgument, "'DaysAfterInitiation' must be a positive integer"));
                }
                if !tags.is_empty() || larger_than.is_some() || smaller_than.is_some() {
                    return Err(s3_error!(
                        InvalidRequest,
                        "AbortIncompleteMultipartUpload cannot be specified with Tags or ObjectSize filters"
                    ));
                }
                Some(i64::from(days))
            }
        };

        let mut transitions = Vec::new();
        for t in r.transitions.iter().flatten() {
            if t.storage_class.is_none() || t.days.is_some() == t.date.is_some() || t.days.is_some_and(|d| d < 0) {
                return Err(malformed());
            }
            let class = target(t.storage_class.as_ref().ok_or_else(malformed)?)?;
            let when = match (&t.days, &t.date) {
                (Some(d), _) => Expiration::Days(i64::from(*d)),
                (_, Some(date)) => {
                    let at = time::OffsetDateTime::from(date.clone());
                    if at.unix_timestamp() % DAY != 0 || at.nanosecond() != 0 {
                        return Err(s3_error!(InvalidArgument, "'Date' must be at midnight GMT"));
                    }
                    Expiration::Date(at.unix_timestamp())
                }
                _ => return Err(malformed()),
            };
            transitions.push((when, class));
        }
        let mut noncurrent_transitions = Vec::new();
        for t in r.noncurrent_version_transitions.iter().flatten() {
            if t.storage_class.is_none() || t.noncurrent_days.is_none_or(|d| d < 0) {
                return Err(malformed());
            }
            if t.newer_noncurrent_versions.is_some_and(|v| v <= 0) {
                return Err(s3_error!(InvalidArgument, "'NewerNoncurrentVersions' must be a positive integer"));
            }
            noncurrent_transitions.push(NoncurrentMove {
                days: i64::from(t.noncurrent_days.unwrap_or_default()),
                newer: t.newer_noncurrent_versions.map(|v| v as usize),
                class: target(t.storage_class.as_ref().ok_or_else(malformed)?)?,
            });
        }

        let enabled = r.status.as_str() == ExpirationStatus::ENABLED;
        Ok(Self {
            id: r.id.clone(),
            prefix,
            tags,
            larger_than,
            smaller_than,
            // Disabled rules are validated like any other but never fire.
            expiration: expiration.filter(|_| enabled),
            expired_delete_marker: expired_delete_marker && enabled,
            noncurrent: noncurrent.filter(|_| enabled),
            abort_days: abort_days.filter(|_| enabled),
            transitions: if enabled { transitions } else { Vec::new() },
            noncurrent_transitions: if enabled { noncurrent_transitions } else { Vec::new() },
        })
    }

    /// Whether the rule's filter selects this key (and, if given, this object).
    fn matches(&self, key: &str, obj: Option<&Object>) -> bool {
        if !key.starts_with(&self.prefix) {
            return false;
        }
        if self.tags.is_empty() && self.larger_than.is_none() && self.smaller_than.is_none() {
            return true;
        }
        let Some(o) = obj else { return false };
        let size = o.data.size as i64;
        self.larger_than.is_none_or(|n| size > n)
            && self.smaller_than.is_none_or(|n| size < n)
            && self.tags.iter().all(|t| o.tags.contains(t))
    }

    /// Whether the default minimum size lets `size` bytes move to `class`. A size filter on the
    /// rule replaces the default.
    fn size_allows(&self, lc: &Lifecycle, size: u64, class: &str) -> bool {
        if self.larger_than.is_some() || self.smaller_than.is_some() || size >= MIN_TRANSITION_SIZE {
            return true;
        }
        let legacy = lc.transition_default_minimum_object_size.as_ref().map(|t| t.as_str()) == Some("varies_by_storage_class");
        legacy && matches!(class, StorageClass::GLACIER | StorageClass::DEEP_ARCHIVE)
    }

    fn expiration_due(&self, created: SystemTime) -> Option<i64> {
        match self.expiration? {
            Expiration::Days(d) => Some(due(created, d)),
            Expiration::Date(at) => Some(at),
        }
    }
}

/// Called on every access to a bucket that has rules: applies them if the UTC day changed.
pub fn tick(bucket: &Bucket, now: SystemTime) {
    let cfg: &BucketConfig = &bucket.config;
    let day = secs(now).div_euclid(DAY);
    if cfg.lifecycle_day.swap(day, Ordering::AcqRel) != day {
        let rules: Option<Arc<Lifecycle>> = cfg.lifecycle.read().clone();
        if let Some(lc) = rules {
            sweep(&lc, bucket, now);
        }
    }
}

pub fn sweep(lc: &Lifecycle, bucket: &Bucket, now: SystemTime) {
    let now_secs = secs(now);
    let mode = *bucket.versioning.read();
    {
        let mut objects = bucket.objects.write();
        let mut emptied = Vec::new();
        for (key, entry) in objects.iter_mut() {
            sweep_entry(lc, key, entry, mode, now_secs, bucket.owner);
            if entry.versions.is_empty() {
                emptied.push(key.clone());
            }
        }
        for key in emptied {
            objects.remove(&key);
        }
    }
    let mut uploads = bucket.uploads.write();
    uploads.retain(|(key, _), up| {
        !lc.rules.iter().any(|r| {
            r.abort_days
                .is_some_and(|d| key.starts_with(&r.prefix) && due(up.initiated, d) <= now_secs)
        })
    });
}

/// `obj` in the deepest of the `(class, transition time)` candidates, if it lies below the
/// object's current class.
fn moved(obj: &Object, candidates: impl Iterator<Item = (&'static str, i64)>) -> Option<Object> {
    let current = obj
        .ext
        .as_ref()
        .and_then(|e| e.storage_class.as_ref())
        .map_or(0, |c| rank(c.as_str()).unwrap_or(0));
    let (class, at) = candidates
        .filter(|(c, _)| rank(c).is_some_and(|r| r > current))
        .max_by_key(|(c, _)| rank(c))?;
    let mut ext = obj.ext.as_deref().cloned().unwrap_or_default();
    ext.storage_class = Some(StorageClass::from_static(class));
    ext.restore = None;
    ext.class_since = Some(UNIX_EPOCH + Duration::from_secs(at.max(0) as u64));
    let mut out = obj.clone();
    out.ext = Some(Arc::new(ext));
    Some(out)
}

fn sweep_entry(lc: &Lifecycle, key: &str, entry: &mut Entry, mode: Versioning, now: i64, bucket_owner: UserId) {
    // The current version first moves down the storage class waterfall (what was due before it
    // expired), then expires.
    let expires_at = entry.current().and_then(|obj| {
        lc.rules
            .iter()
            .filter(|r| r.matches(key, Some(obj)))
            .filter_map(|r| r.expiration_due(obj.last_modified))
            .min()
    });
    if let Some(latest) = entry.latest()
        && let Some(obj) = &latest.obj
        && let Some(moved) = lc.transition_current(key, obj, now, expires_at)
    {
        let last = entry.versions.len() - 1;
        entry.versions[last] = Arc::new(Version {
            id: latest.id.clone(),
            obj: Some(moved),
            last_modified: latest.last_modified,
            owner: latest.owner,
        });
    }

    // 1. Expire the current version.
    if let Some(at) = expires_at
        && at <= now
    {
        match mode {
            Versioning::Unversioned => entry.versions.clear(),
            Versioning::Enabled | Versioning::Suspended => {
                if mode == Versioning::Suspended {
                    entry.versions.retain(|v| v.id != "null");
                }
                entry.versions.push(Arc::new(Version {
                    id: if mode == Versioning::Enabled {
                        new_version_id()
                    } else {
                        "null".to_owned()
                    },
                    obj: None,
                    // The marker appears when the rule fires, not when we got around to it.
                    last_modified: UNIX_EPOCH + Duration::from_secs(at.max(0) as u64),
                    // Nobody asked for it: the bucket owner's rule did.
                    owner: bucket_owner,
                }));
            }
        }
    }

    // 2. Remove noncurrent versions that have been noncurrent long enough.
    if entry.versions.len() > 1 {
        let last = entry.versions.len() - 1;
        let locked_at = UNIX_EPOCH + Duration::from_secs(now.max(0) as u64);
        let mut remove = Vec::new();
        for (i, v) in entry.versions[..last].iter().enumerate() {
            let became_noncurrent = entry.versions[i + 1].last_modified;
            // Noncurrent versions newer than this one (the current version does not count).
            let newer = last - i - 1;
            // Object Lock holds a version past its rule until the retention ends.
            if v.obj.as_ref().is_some_and(|o| crate::objext::delete_blocked(o, locked_at, false)) {
                continue;
            }
            let expired = lc.rules.iter().any(|r| {
                r.noncurrent.as_ref().is_some_and(|n| {
                    r.matches(key, v.obj.as_ref()) && due(became_noncurrent, n.days) <= now && n.newer.is_none_or(|keep| newer >= keep)
                })
            });
            if expired {
                remove.push(i);
            }
        }
        for i in remove.into_iter().rev() {
            entry.versions.remove(i);
        }
    }

    // Same for noncurrent versions that have been noncurrent long enough.
    if lc.rules.iter().any(|r| !r.noncurrent_transitions.is_empty()) && entry.versions.len() > 1 {
        let last = entry.versions.len() - 1;
        for i in 0..last {
            let v = &entry.versions[i];
            let Some(obj) = &v.obj else { continue };
            let became_noncurrent = entry.versions[i + 1].last_modified;
            let newer = last - i - 1;
            if let Some(moved) = lc.transition_noncurrent(key, obj, became_noncurrent, newer, now) {
                entry.versions[i] = Arc::new(Version {
                    id: v.id.clone(),
                    obj: Some(moved),
                    last_modified: v.last_modified,
                    owner: v.owner,
                });
            }
        }
    }

    // 3. A delete marker with nothing behind it is clutter.
    if entry.versions.len() == 1
        && entry.versions[0].obj.is_none()
        && lc.rules.iter().any(|r| r.expired_delete_marker && key.starts_with(&r.prefix))
    {
        entry.versions.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{ObjectData, ObjectMeta};
    use s3s::dto::{AbortIncompleteMultipartUpload, LifecycleExpiration, LifecycleRuleFilter, NoncurrentVersionExpiration};

    fn rule(f: impl FnOnce(&mut LifecycleRule)) -> LifecycleRule {
        let mut r = LifecycleRule {
            abort_incomplete_multipart_upload: None,
            expiration: None,
            filter: Some(LifecycleRuleFilter::default()),
            id: None,
            noncurrent_version_expiration: None,
            noncurrent_version_transitions: None,
            prefix: None,
            status: ExpirationStatus::from_static(ExpirationStatus::ENABLED),
            transitions: None,
        };
        f(&mut r);
        r
    }

    fn lc(rules: Vec<LifecycleRule>) -> S3Result<Lifecycle> {
        Lifecycle::new(BucketLifecycleConfiguration { rules })
    }

    fn at(day: i64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs((day * DAY) as u64)
    }

    fn object(created: SystemTime) -> Object {
        Object {
            data: ObjectData::default(),
            meta: ObjectMeta::default(),
            e_tag: String::new(),
            last_modified: created,
            checksum: Default::default(),
            checksum_type: None,
            parts_count: None,
            part_sizes: None,
            tags: Vec::new(),
            acl: None,
            ext: None,
        }
    }

    fn version(id: &str, obj: Option<Object>, at: SystemTime) -> Arc<Version> {
        Arc::new(Version {
            id: id.into(),
            obj,
            last_modified: at,
            owner: UserId::DEFAULT,
        })
    }

    #[test]
    fn expiration_rounds_up_to_midnight() {
        // Created mid-day 0, 1 day: due at the end of day 1, i.e. midnight starting day 2.
        let created = at(0) + Duration::from_secs(3600);
        assert_eq!(due(created, 1), 2 * DAY);
        assert_eq!(due(at(0), 1), DAY);
    }

    #[test]
    fn unversioned_expiry() {
        let l = lc(vec![rule(|r| {
            r.expiration = Some(LifecycleExpiration {
                days: Some(1),
                ..Default::default()
            })
        })])
        .unwrap();
        let mut e = Entry::default();
        e.versions
            .push(version("null", Some(object(at(0) + Duration::from_secs(10))), at(0)));
        sweep_entry(&l, "k", &mut e, Versioning::Unversioned, DAY, UserId::DEFAULT);
        assert_eq!(e.versions.len(), 1);
        sweep_entry(&l, "k", &mut e, Versioning::Unversioned, 2 * DAY, UserId::DEFAULT);
        assert!(e.versions.is_empty());
    }

    #[test]
    fn versioned_expiry_creates_marker_then_cleans_up() {
        let l = lc(vec![
            rule(|r| {
                r.expiration = Some(LifecycleExpiration {
                    days: Some(1),
                    ..Default::default()
                });
                r.noncurrent_version_expiration = Some(NoncurrentVersionExpiration {
                    noncurrent_days: Some(1),
                    newer_noncurrent_versions: None,
                });
            }),
            rule(|r| {
                r.expiration = Some(LifecycleExpiration {
                    expired_object_delete_marker: Some(true),
                    ..Default::default()
                })
            }),
        ])
        .unwrap();
        let mut e = Entry::default();
        e.versions.push(version("v1", Some(object(at(0))), at(0)));
        sweep_entry(&l, "k", &mut e, Versioning::Enabled, DAY, UserId::DEFAULT);
        assert_eq!(e.versions.len(), 2);
        assert!(e.versions[1].obj.is_none());
        // Not yet noncurrent for a full day.
        sweep_entry(&l, "k", &mut e, Versioning::Enabled, DAY, UserId::DEFAULT);
        assert_eq!(e.versions.len(), 2);
        // Well past both deadlines: the old version goes, then the lone marker goes with it.
        sweep_entry(&l, "k", &mut e, Versioning::Enabled, 5 * DAY, UserId::DEFAULT);
        assert!(e.versions.is_empty());
    }

    #[test]
    fn newer_noncurrent_versions_are_kept() {
        let l = lc(vec![rule(|r| {
            r.noncurrent_version_expiration = Some(NoncurrentVersionExpiration {
                noncurrent_days: Some(1),
                newer_noncurrent_versions: Some(2),
            })
        })])
        .unwrap();
        let mut e = Entry::default();
        for i in 0..5 {
            e.versions.push(version(&format!("v{i}"), Some(object(at(i))), at(i)));
        }
        sweep_entry(&l, "k", &mut e, Versioning::Enabled, 100 * DAY, UserId::DEFAULT);
        let ids: Vec<_> = e.versions.iter().map(|v| v.id.clone()).collect();
        assert_eq!(ids, ["v2", "v3", "v4"], "current plus the 2 newest noncurrent stay");
    }

    #[test]
    fn filters() {
        let l = lc(vec![rule(|r| {
            r.filter = Some(LifecycleRuleFilter {
                prefix: Some("logs/".into()),
                ..Default::default()
            });
            r.expiration = Some(LifecycleExpiration {
                days: Some(1),
                ..Default::default()
            });
        })])
        .unwrap();
        let mut keep = Entry::default();
        keep.versions.push(version("null", Some(object(at(0))), at(0)));
        sweep_entry(&l, "data/x", &mut keep, Versioning::Unversioned, 10 * DAY, UserId::DEFAULT);
        assert_eq!(keep.versions.len(), 1);
        sweep_entry(&l, "logs/x", &mut keep, Versioning::Unversioned, 10 * DAY, UserId::DEFAULT);
        assert!(keep.versions.is_empty());
    }

    #[test]
    fn validation() {
        let days = |d| LifecycleExpiration {
            days: Some(d),
            ..Default::default()
        };
        assert!(lc(vec![]).is_err());
        assert!(lc(vec![rule(|r| r.expiration = Some(days(0)))]).is_err());
        assert!(lc(vec![rule(|_| {})]).is_err(), "no action");
        assert!(lc(vec![rule(|r| r.expiration = Some(days(1))), rule(|r| r.expiration = Some(days(2)))]).is_ok());
        let dup = |r: &mut LifecycleRule| {
            r.id = Some("same".into());
            r.expiration = Some(days(1));
        };
        assert!(lc(vec![rule(dup), rule(dup)]).is_err());
        assert!(
            lc(vec![rule(|r| {
                r.filter = None;
                r.expiration = Some(days(1));
            })])
            .is_err(),
            "neither filter nor prefix"
        );
        assert!(
            lc(vec![rule(|r| r.abort_incomplete_multipart_upload =
                Some(AbortIncompleteMultipartUpload {
                    days_after_initiation: Some(7)
                }))])
            .is_ok()
        );
    }

    #[test]
    fn expiration_header_names_the_rule() {
        let l = lc(vec![rule(|r| {
            r.id = Some("r1".into());
            r.expiration = Some(LifecycleExpiration {
                days: Some(3),
                ..Default::default()
            });
        })])
        .unwrap();
        let h = l.expiration_header("k", &object(at(0))).unwrap();
        assert_eq!(h, "expiry-date=\"Sun, 04 Jan 1970 00:00:00 GMT\", rule-id=\"r1\"");
    }
}
