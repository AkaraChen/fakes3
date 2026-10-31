//! Archive storage classes: `GLACIER` (S3 Glacier Flexible Retrieval), `DEEP_ARCHIVE` and the
//! Archive Access / Deep Archive Access tiers of `INTELLIGENT_TIERING`.
//!
//! An archived object keeps its data, but reads that need the data (GET, the source of CopyObject
//! and UploadPartCopy) fail with 403 `InvalidObjectState` until a restored copy exists. HEAD and
//! the other metadata operations keep working. `RestoreObject` records a [`Restore`] in the
//! object's [`ObjectExt`]; nothing runs in the background. Every read compares the recorded times
//! with the server clock, so `fakes3 ctl clock advance` is all it takes for a restore to finish
//! and, later, for the restored copy to expire.
//!
//! Completion times are fixed per class and tier (the lower bound of what AWS documents):
//!
//! | class                                     | Expedited | Standard | Bulk |
//! |-------------------------------------------|-----------|----------|------|
//! | `GLACIER`, IT Archive Access              | 1 minute  | 3 hours  | 5 h  |
//! | `DEEP_ARCHIVE`, IT Deep Archive Access    | rejected  | 12 hours | 48 h |
//!
//! A restored Glacier copy lives for `Days` days counted from completion, rounded up to midnight
//! UTC. A restored Intelligent-Tiering object moves back to the Frequent Access tier instead, so
//! it has no expiry; its idle days start counting again from completion.
//!
//! Buckets can hold Intelligent-Tiering configurations (`PutBucketIntelligentTieringConfiguration`).
//! An `INTELLIGENT_TIERING` object counts as being in the Archive Access (or Deep Archive Access)
//! tier once it is older than the matching tiering's `Days`, measured from the later of its
//! creation, its lifecycle transition and its last restore. fakes3 does not track reads, so
//! reading an object does not reset the count the way "access" does in AWS.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use http::StatusCode;
use s3s::dto::*;
use s3s::{S3Error, S3ErrorCode, S3Request, S3Response, S3Result, s3_error};

use crate::bucket_config::custom_error;
use crate::lifecycle::{due, http_date};
use crate::objext::ObjectExt;
use crate::s3impl::FakeS3;
use crate::store::{Bucket, Object as StoreObject};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum RestoreTier {
    Bulk,
    Standard,
    Expedited,
}

impl RestoreTier {
    fn parse(t: &Tier) -> S3Result<Self> {
        match t.as_str() {
            Tier::BULK => Ok(Self::Bulk),
            Tier::STANDARD => Ok(Self::Standard),
            Tier::EXPEDITED => Ok(Self::Expedited),
            _ => Err(malformed()),
        }
    }
}

/// s3s answers RestoreObject with 200 whatever the handler says; `lib.rs` applies this to the
/// response instead (202 for a new request, 200 for a copy that is already restored).
#[derive(Clone, Copy)]
pub struct StatusOverride(pub StatusCode);

/// A restore request on an object version. Which phase it is in follows from the clock.
#[derive(Clone, Copy, Debug)]
pub struct Restore {
    /// When the retrieval completes.
    pub ready: SystemTime,
    /// When the restored copy is deleted again. Equal to `ready` for Intelligent-Tiering, whose
    /// restored objects stay in the Frequent Access tier.
    pub expires: SystemTime,
    pub tier: RestoreTier,
}

/// Where an archived object sits.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Class {
    Glacier,
    DeepArchive,
    ItArchive,
    ItDeepArchive,
}

impl Class {
    fn deep(self) -> bool {
        matches!(self, Self::DeepArchive | Self::ItDeepArchive)
    }

    fn intelligent(self) -> bool {
        matches!(self, Self::ItArchive | Self::ItDeepArchive)
    }

    fn retrieval_time(self, tier: RestoreTier) -> Option<Duration> {
        const MIN: u64 = 60;
        const HOUR: u64 = 3600;
        Some(Duration::from_secs(match (self.deep(), tier) {
            (false, RestoreTier::Expedited) => MIN,
            (false, RestoreTier::Standard) => 3 * HOUR,
            (false, RestoreTier::Bulk) => 5 * HOUR,
            (true, RestoreTier::Expedited) => return None,
            (true, RestoreTier::Standard) => 12 * HOUR,
            (true, RestoreTier::Bulk) => 48 * HOUR,
        }))
    }

    fn access_tier(self) -> Option<&'static str> {
        match self {
            Self::ItArchive => Some(ArchiveStatus::ARCHIVE_ACCESS),
            Self::ItDeepArchive => Some(ArchiveStatus::DEEP_ARCHIVE_ACCESS),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Ongoing,
    /// A restored copy is readable until this time.
    Restored(SystemTime),
}

/// An object whose data is in an archive class or tier right now.
#[derive(Clone, Copy, Debug)]
pub struct Archived {
    pub class: Class,
    /// `None` when there is no restore, or it has expired.
    pub status: Option<Status>,
}

impl Archived {
    fn readable(&self) -> bool {
        matches!(self.status, Some(Status::Restored(_)))
    }

    /// `source` words the error the way S3 does for the source of a copy.
    pub fn require_readable(&self, source: bool) -> S3Result<()> {
        if self.readable() {
            return Ok(());
        }
        let msg = match (source, self.class.intelligent()) {
            (true, _) => "Operation is not valid for the source object's storage class",
            (false, false) => "The action is not valid for the object's storage class",
            (false, true) => "The action is not valid for the object's access tier",
        };
        Err(S3Error::with_message(S3ErrorCode::InvalidObjectState, msg))
    }

    /// The `x-amz-restore` header.
    pub fn restore_header(&self) -> Option<String> {
        match self.status? {
            Status::Ongoing => Some("ongoing-request=\"true\"".to_owned()),
            Status::Restored(at) => Some(format!("ongoing-request=\"false\", expiry-date=\"{}\"", http_date(secs(at))?)),
        }
    }

    /// `RestoreStatus` of a listing entry; nothing for an object that was never restored.
    pub fn restore_status(&self) -> Option<RestoreStatus> {
        Some(match self.status? {
            Status::Ongoing => RestoreStatus {
                is_restore_in_progress: Some(true),
                restore_expiry_date: None,
            },
            Status::Restored(at) => RestoreStatus {
                is_restore_in_progress: Some(false),
                restore_expiry_date: Some(Timestamp::from(at)),
            },
        })
    }

    pub fn archive_status(&self) -> Option<ArchiveStatus> {
        self.class.access_tier().map(ArchiveStatus::from_static)
    }
}

fn secs(t: SystemTime) -> i64 {
    crate::lifecycle::secs(t)
}

fn at(secs: i64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(secs.max(0) as u64)
}

fn malformed() -> S3Error {
    s3_error!(
        MalformedXML,
        "The XML you provided was not well-formed or did not validate against our published schema"
    )
}

fn forbidden(code: &'static str, msg: &'static str) -> S3Error {
    custom_error(code, StatusCode::FORBIDDEN, msg)
}

/// Whether the object's storage class can put it in an archive tier at all.
fn candidate(ext: &ObjectExt) -> bool {
    matches!(
        ext.storage_class.as_ref().map(StorageClass::as_str),
        Some(StorageClass::GLACIER | StorageClass::DEEP_ARCHIVE | StorageClass::INTELLIGENT_TIERING)
    )
}

/// The archive state of `o` at `now`, or `None` when its data is readable without a restore.
pub fn archived(b: &Bucket, key: &str, o: &StoreObject, now: SystemTime) -> Option<Archived> {
    let ext = o.ext.as_deref().filter(|e| candidate(e))?;
    let class = match ext.storage_class.as_ref()?.as_str() {
        StorageClass::GLACIER => Class::Glacier,
        StorageClass::DEEP_ARCHIVE => Class::DeepArchive,
        _ => it_class(b, key, o, ext, now)?,
    };
    let status = ext.restore.and_then(|r| {
        if now < r.ready {
            Some(Status::Ongoing)
        } else if now < r.expires {
            Some(Status::Restored(r.expires))
        } else {
            None
        }
    });
    Some(Archived { class, status })
}

/// The Intelligent-Tiering archive tier an object has idled into, if any.
fn it_class(b: &Bucket, key: &str, o: &StoreObject, ext: &ObjectExt, now: SystemTime) -> Option<Class> {
    let configs = b.config.intelligent_tiering.read();
    if configs.is_empty() {
        return None;
    }
    let mut since = o.last_modified.max(ext.class_since.unwrap_or(UNIX_EPOCH));
    if let Some(r) = ext.restore.filter(|r| r.ready <= now) {
        since = since.max(r.ready);
    }
    let idle_days = (secs(now) - secs(since)).div_euclid(86_400);
    let mut class = None;
    for c in configs
        .values()
        .filter(|c| c.status.as_str() == IntelligentTieringStatus::ENABLED && filter_matches(c, key, o))
    {
        for t in &c.tierings {
            if i64::from(t.days) > idle_days {
                continue;
            }
            match t.access_tier.as_str() {
                IntelligentTieringAccessTier::DEEP_ARCHIVE_ACCESS => class = Some(Class::ItDeepArchive),
                _ if class.is_none() => class = Some(Class::ItArchive),
                _ => {}
            }
        }
    }
    class
}

fn filter_matches(c: &IntelligentTieringConfiguration, key: &str, o: &StoreObject) -> bool {
    let Some(f) = &c.filter else { return true };
    let tag = |t: &Tag| {
        let (k, v) = (t.key.clone().unwrap_or_default(), t.value.clone().unwrap_or_default());
        o.tags.iter().any(|(ok, ov)| *ok == k && *ov == v)
    };
    if let Some(and) = &f.and {
        return and.prefix.as_deref().is_none_or(|p| key.starts_with(p)) && and.tags.iter().flatten().all(tag);
    }
    f.prefix.as_deref().is_none_or(|p| key.starts_with(p)) && f.tag.as_ref().is_none_or(tag)
}

impl FakeS3 {
    /// The archive state of an object read from `bucket`; costs nothing for objects that cannot
    /// be archived. The bucket has been looked up (and swept) by the request already.
    pub(crate) fn archive_state(&self, bucket: &str, key: &str, o: &StoreObject) -> Option<Archived> {
        if !o.ext.as_deref().is_some_and(candidate) {
            return None;
        }
        archived(&*self.store.bucket(bucket)?, key, o, self.clock.now())
    }

    /// Fails reads of archived data that has no restored copy.
    pub(crate) fn require_restored(&self, bucket: &str, key: &str, o: &StoreObject, source: bool) -> S3Result<()> {
        match self.archive_state(bucket, key, o) {
            Some(a) => a.require_readable(source),
            None => Ok(()),
        }
    }

    /// The source of CopyObject / UploadPartCopy must be readable too.
    pub(crate) fn require_copy_source_restored(&self, src: &CopySource, o: &StoreObject) -> S3Result<()> {
        match src {
            CopySource::Bucket { bucket, key, .. } => self.require_restored(bucket, key, o, true),
            _ => Ok(()),
        }
    }

    /// Fills `RestoreStatus` of ListObjectsV2 entries when the request asked for it.
    pub(crate) fn add_restore_status(&self, bucket: &Bucket, contents: &mut [Object]) {
        let now = self.clock.now();
        let objects = bucket.objects.read();
        for c in contents.iter_mut() {
            let archivable = matches!(
                c.storage_class.as_ref().map(ObjectStorageClass::as_str),
                Some(ObjectStorageClass::GLACIER | ObjectStorageClass::DEEP_ARCHIVE | ObjectStorageClass::INTELLIGENT_TIERING)
            );
            let Some(key) = c.key.as_deref().filter(|_| archivable) else {
                continue;
            };
            if let Some(o) = objects.get(key).and_then(|e| e.current()) {
                c.restore_status = archived(bucket, key, o, now).and_then(|a| a.restore_status());
            }
        }
    }

    pub(crate) async fn do_restore_object(&self, req: S3Request<RestoreObjectInput>) -> S3Result<S3Response<RestoreObjectOutput>> {
        let input = req.input;
        let b = self.bucket(&input.bucket)?;
        let rr = input.restore_request.ok_or_else(malformed)?;
        if rr.type_.is_some() || rr.select_parameters.is_some() || rr.output_location.is_some() {
            return Err(s3_error!(
                NotImplemented,
                "SELECT restore requests (Type, SelectParameters, OutputLocation) are not supported"
            ));
        }
        let tier = match rr.glacier_job_parameters.as_ref().map(|g| &g.tier).or(rr.tier.as_ref()) {
            Some(t) => RestoreTier::parse(t)?,
            None => RestoreTier::Standard,
        };
        if rr.days.is_some_and(|d| d <= 0) {
            return Err(s3_error!(InvalidArgument, "Days must be a positive integer"));
        }
        let now = self.clock.now();
        let mut status = StatusCode::ACCEPTED;
        self.modify_object(&b, &input.key, input.version_id.as_deref(), |o| {
            let Some(a) = archived(&b, &input.key, o, now) else {
                return Err(forbidden(
                    "ObjectAlreadyInActiveTierError",
                    "This operation is not allowed against this storage tier",
                ));
            };
            let wait = a.class.retrieval_time(tier).ok_or_else(|| {
                s3_error!(
                    InvalidArgument,
                    "Expedited retrievals are not available for the Deep Archive storage class or tier"
                )
            })?;
            // Restored Intelligent-Tiering objects go back to Frequent Access: no lifetime.
            let days = if a.class.intelligent() {
                None
            } else {
                Some(i64::from(rr.days.ok_or_else(malformed)?))
            };
            let finish = |ready: SystemTime| days.map_or(ready, |d| at(due(ready, d)));
            let old = o.ext.as_ref().and_then(|e| e.restore);
            let restore = match (a.status, old) {
                // Already restored: only the lifetime changes, counted from now.
                (Some(Status::Restored(_)), Some(old)) => {
                    status = StatusCode::OK;
                    Restore {
                        expires: finish(now),
                        ..old
                    }
                }
                // A faster tier speeds an ongoing restore up; anything else has to wait.
                (Some(Status::Ongoing), Some(old)) if tier > old.tier => {
                    let ready = old.ready.min(now + wait);
                    Restore {
                        ready,
                        expires: finish(ready),
                        tier,
                    }
                }
                (Some(_), _) => {
                    return Err(custom_error(
                        "RestoreAlreadyInProgress",
                        StatusCode::CONFLICT,
                        "Object restore is already in progress",
                    ));
                }
                (None, _) => Restore {
                    ready: now + wait,
                    expires: finish(now + wait),
                    tier,
                },
            };
            let mut ext = o.ext.as_deref().cloned().unwrap_or_default();
            ext.restore = Some(restore);
            o.ext = Some(std::sync::Arc::new(ext));
            Ok(())
        })?;
        let mut resp = S3Response::new(RestoreObjectOutput::default());
        resp.extensions.insert(StatusOverride(status));
        Ok(resp)
    }

    // ---- Intelligent-Tiering configurations ----

    pub(crate) fn do_put_it_config(
        &self,
        req: S3Request<PutBucketIntelligentTieringConfigurationInput>,
    ) -> S3Result<S3Response<PutBucketIntelligentTieringConfigurationOutput>> {
        let input = req.input;
        let b = self.bucket(&input.bucket)?;
        let c = input.intelligent_tiering_configuration;
        if c.id != input.id {
            return Err(s3_error!(
                InvalidArgument,
                "ID in the request body must match the ID in the query string"
            ));
        }
        if !matches!(
            c.status.as_str(),
            IntelligentTieringStatus::ENABLED | IntelligentTieringStatus::DISABLED
        ) || c.tierings.is_empty()
        {
            return Err(malformed());
        }
        let mut seen = [false; 2];
        for t in &c.tierings {
            let (slot, min) = match t.access_tier.as_str() {
                IntelligentTieringAccessTier::ARCHIVE_ACCESS => (0, 90),
                IntelligentTieringAccessTier::DEEP_ARCHIVE_ACCESS => (1, 180),
                _ => return Err(malformed()),
            };
            if !(min..=730).contains(&t.days) {
                return Err(s3_error!(
                    InvalidArgument,
                    "Days must be between {min} and 730 for the {} tier",
                    t.access_tier.as_str()
                ));
            }
            if std::mem::replace(&mut seen[slot], true) {
                return Err(s3_error!(InvalidArgument, "Each access tier can only be configured once"));
            }
        }
        b.config.intelligent_tiering.write().insert(input.id, c);
        Ok(S3Response::new(PutBucketIntelligentTieringConfigurationOutput {}))
    }

    pub(crate) fn do_get_it_config(
        &self,
        req: S3Request<GetBucketIntelligentTieringConfigurationInput>,
    ) -> S3Result<S3Response<GetBucketIntelligentTieringConfigurationOutput>> {
        let b = self.bucket(&req.input.bucket)?;
        let c = b
            .config
            .intelligent_tiering
            .read()
            .get(&req.input.id)
            .cloned()
            .ok_or_else(no_such_config)?;
        Ok(S3Response::new(GetBucketIntelligentTieringConfigurationOutput {
            intelligent_tiering_configuration: Some(c),
        }))
    }

    pub(crate) fn do_list_it_configs(
        &self,
        req: S3Request<ListBucketIntelligentTieringConfigurationsInput>,
    ) -> S3Result<S3Response<ListBucketIntelligentTieringConfigurationsOutput>> {
        let b = self.bucket(&req.input.bucket)?;
        let list: Vec<_> = b.config.intelligent_tiering.read().values().cloned().collect();
        Ok(S3Response::new(ListBucketIntelligentTieringConfigurationsOutput {
            continuation_token: req.input.continuation_token,
            intelligent_tiering_configuration_list: (!list.is_empty()).then_some(list),
            is_truncated: Some(false),
            next_continuation_token: None,
        }))
    }

    pub(crate) fn do_delete_it_config(
        &self,
        req: S3Request<DeleteBucketIntelligentTieringConfigurationInput>,
    ) -> S3Result<S3Response<DeleteBucketIntelligentTieringConfigurationOutput>> {
        let b = self.bucket(&req.input.bucket)?;
        b.config
            .intelligent_tiering
            .write()
            .remove(&req.input.id)
            .ok_or_else(no_such_config)?;
        Ok(S3Response::new(DeleteBucketIntelligentTieringConfigurationOutput {}))
    }
}

fn no_such_config() -> S3Error {
    custom_error(
        "NoSuchConfiguration",
        StatusCode::NOT_FOUND,
        "The specified configuration does not exist.",
    )
}

/// A bucket's Intelligent-Tiering configurations by id.
pub type ItConfigs = BTreeMap<String, IntelligentTieringConfiguration>;
