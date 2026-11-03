//! In-memory object store.
//!
//! Object data is a list of immutable, reference-counted `Bytes` segments. Small payloads live on
//! the heap; payloads at or above the inline threshold are written into a `memfd` (tmpfs-backed,
//! freed automatically when the last mapping drops) and served from a read-only mapping. Either
//! way a read is a slice of `Bytes`, so GET, CopyObject and CompleteMultipartUpload never copy
//! object data.

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::Write;
use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use parking_lot::RwLock;

use crate::memory::{LimitExceeded, MemoryTracker, Reservation, Tracked};
use crate::objext::{BucketCfg, ObjectExt};
use crate::users::UserId;

#[derive(Clone, Default)]
pub struct ObjectData {
    pub segments: Arc<[Bytes]>,
    pub size: u64,
}

impl ObjectData {
    pub fn from_segments(segments: Vec<Bytes>) -> Self {
        let size = segments.iter().map(|s| s.len() as u64).sum();
        Self {
            segments: segments.into(),
            size,
        }
    }

    /// Returns the segments covering `range`, sliced without copying.
    pub fn slice(&self, range: std::ops::Range<u64>) -> Vec<Bytes> {
        let mut out = Vec::new();
        let mut offset = 0u64;
        for seg in self.segments.iter() {
            let seg_start = offset;
            let seg_end = offset + seg.len() as u64;
            offset = seg_end;
            if seg_end <= range.start {
                continue;
            }
            if seg_start >= range.end {
                break;
            }
            let lo = range.start.max(seg_start) - seg_start;
            let hi = range.end.min(seg_end) - seg_start;
            out.push(seg.slice(lo as usize..hi as usize));
        }
        out
    }
}

#[derive(Clone, Default)]
pub struct ObjectMeta {
    pub content_type: Option<String>,
    pub content_encoding: Option<String>,
    pub content_disposition: Option<String>,
    pub content_language: Option<String>,
    pub cache_control: Option<String>,
    pub expires: Option<String>,
    pub user_metadata: Option<HashMap<String, String>>,
}

#[derive(Clone)]
pub struct Object {
    pub data: ObjectData,
    pub meta: ObjectMeta,
    pub e_tag: String,
    pub last_modified: SystemTime,
    pub checksum: s3s::dto::Checksum,
    pub checksum_type: Option<s3s::dto::ChecksumType>,
    pub parts_count: Option<i32>,
    /// Sizes of the parts a multipart object was assembled from, for `?partNumber=` reads.
    pub part_sizes: Option<Arc<[u64]>>,
    pub tags: Vec<(String, String)>,
    /// Storage class, encryption, Object Lock...; `None` for a plain object.
    pub ext: Option<Arc<ObjectExt>>,
    /// `None` is the default `private` ACL.
    pub acl: Option<Arc<crate::acl::Acl>>,
}

/// One entry in a key's version chain. `obj == None` is a delete marker.
pub struct Version {
    pub id: String,
    pub obj: Option<Object>,
    pub last_modified: SystemTime,
    /// The account that owns the object (see `docs/multi-user.md`); for a delete marker, the one
    /// that created it.
    pub owner: UserId,
}

/// All versions of a key, oldest first. Unversioned buckets keep a single `null` version.
#[derive(Clone, Default)]
pub struct Entry {
    pub versions: Vec<Arc<Version>>,
}

impl Entry {
    pub fn latest(&self) -> Option<&Arc<Version>> {
        self.versions.last()
    }

    /// The object a plain GET sees: the latest version, unless it is a delete marker.
    pub fn current(&self) -> Option<&Object> {
        self.latest().and_then(|v| v.obj.as_ref())
    }

    pub fn find(&self, id: &str) -> Option<&Arc<Version>> {
        self.versions.iter().rev().find(|v| v.id == id)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Versioning {
    Unversioned,
    Enabled,
    Suspended,
}

#[derive(Clone)]
pub struct Part {
    pub data: ObjectData,
    pub md5: [u8; 16],
    pub last_modified: SystemTime,
    pub checksum: s3s::dto::Checksum,
}

#[derive(Clone)]
pub struct Upload {
    /// Who sent CreateMultipartUpload.
    pub initiator: UserId,
    /// Who will own the object, decided like a PutObject's owner when the upload starts.
    pub owner: UserId,
    pub meta: ObjectMeta,
    pub tags: Vec<(String, String)>,
    pub ext: Option<ObjectExt>,
    pub acl: Option<Arc<crate::acl::Acl>>,
    pub initiated: SystemTime,
    pub checksum_algorithm: Option<s3s::dto::ChecksumAlgorithm>,
    /// Effective checksum type (explicit, or the default for the algorithm).
    pub checksum_type: s3s::dto::ChecksumType,
    pub parts: BTreeMap<i32, Part>,
}

/// A bucket's key index: key to version chain, ordered for listing.
pub type Index = BTreeMap<String, Entry>;

pub struct Bucket {
    pub created: SystemTime,
    /// The account that created the bucket.
    pub owner: UserId,
    pub versioning: RwLock<Versioning>,
    pub tags: RwLock<Option<Vec<(String, String)>>>,
    pub objects: RwLock<Index>,
    pub uploads: RwLock<BTreeMap<(String, String), Upload>>,
    pub cfg: BucketCfg,
    pub config: crate::bucket_config::BucketConfig,
}

#[derive(Default)]
pub struct Store {
    pub buckets: RwLock<BTreeMap<String, Arc<Bucket>>>,
    pub access: crate::bucket_config::AccessState,
}

impl Store {
    pub fn bucket(&self, name: &str) -> Option<Arc<Bucket>> {
        self.buckets.read().get(name).cloned()
    }

    /// Adds `bucket` under `name`. A bucket its owner already has is left as it is, which is what
    /// us-east-1 answers 200 for (the legacy behaviour in the CreateBucket error list; other
    /// regions say BucketAlreadyOwnedByYou). Returns false when another account owns the name.
    pub fn add_bucket(&self, name: String, bucket: Bucket) -> bool {
        match self.buckets.write().entry(name) {
            std::collections::btree_map::Entry::Occupied(e) => e.get().owner == bucket.owner,
            std::collections::btree_map::Entry::Vacant(e) => {
                e.insert(Arc::new(bucket));
                true
            }
        }
    }
}

impl Bucket {
    pub fn new(now: SystemTime, owner: UserId) -> Self {
        Self {
            created: now,
            owner,
            versioning: RwLock::new(Versioning::Unversioned),
            tags: RwLock::default(),
            objects: RwLock::default(),
            uploads: RwLock::default(),
            cfg: BucketCfg::default(),
            config: Default::default(),
        }
    }
}

pub fn new_version_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

impl Bucket {
    /// Stores `obj` as the newest version of `key` and returns the version id to report: only
    /// buckets with versioning Enabled report one (a suspended bucket writes the `null` version
    /// silently, like the PutObject reference's suspended-bucket example).
    pub fn put(&self, objects: &mut Index, key: String, obj: Object, owner: UserId) -> Option<String> {
        let mode = *self.versioning.read();
        let entry = objects.entry(key).or_default();
        let id = match mode {
            Versioning::Enabled => new_version_id(),
            _ => {
                entry.versions.retain(|v| v.id != "null");
                "null".to_owned()
            }
        };
        let last_modified = obj.last_modified;
        entry.versions.push(Arc::new(Version {
            id: id.clone(),
            obj: Some(obj),
            last_modified,
            owner,
        }));
        (mode == Versioning::Enabled).then_some(id)
    }

    /// A DELETE without a version id. Returns the id of the delete marker created, if any.
    pub fn delete_current(&self, objects: &mut Index, key: &str, now: SystemTime, owner: UserId) -> Option<String> {
        let mode = *self.versioning.read();
        if mode == Versioning::Unversioned {
            objects.remove(key);
            return None;
        }
        let entry = objects.entry(key.to_owned()).or_default();
        let id = if mode == Versioning::Enabled {
            new_version_id()
        } else {
            entry.versions.retain(|v| v.id != "null");
            "null".to_owned()
        };
        entry.versions.push(Arc::new(Version {
            id: id.clone(),
            obj: None,
            last_modified: now,
            owner,
        }));
        Some(id)
    }

    /// Permanently removes one version. Returns whether it was a delete marker, or `None` if absent.
    pub fn delete_version(objects: &mut Index, key: &str, id: &str) -> Option<bool> {
        let entry = objects.get_mut(key)?;
        let pos = entry.versions.iter().position(|v| v.id == id)?;
        let removed = entry.versions.remove(pos);
        if entry.versions.is_empty() {
            objects.remove(key);
        }
        Some(removed.obj.is_none())
    }
}

/// Why a blob could not be written.
#[derive(Debug)]
pub enum BlobError {
    Limit(LimitExceeded),
    Io(std::io::Error),
}

impl From<std::io::Error> for BlobError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<LimitExceeded> for BlobError {
    fn from(e: LimitExceeded) -> Self {
        Self::Limit(e)
    }
}

/// Accumulates an incoming body, spilling to a memfd once it reaches `threshold` bytes.
///
/// Small bodies are copied into an exact-size heap buffer rather than keeping the request's
/// chunks, which would pin hyper's (much larger) read buffers.
///
/// Memory is charged to the tracker up front when the size is known (so an oversized request is
/// refused before its body is read) and chunk by chunk otherwise. Dropping the writer unfinished
/// releases everything; finishing hands the charge to the returned `Bytes`.
pub struct BlobWriter {
    threshold: usize,
    reservation: Reservation,
    len: u64,
    mem: Vec<u8>,
    file: Option<File>,
}

impl BlobWriter {
    pub fn new(memory: &Arc<MemoryTracker>, threshold: usize, size_hint: Option<u64>) -> Result<Self, BlobError> {
        let mut w = Self {
            threshold,
            reservation: memory.reserve(size_hint.unwrap_or(0))?,
            len: 0,
            mem: Vec::new(),
            file: None,
        };
        match size_hint {
            Some(n) if n as usize >= threshold => w.file = Some(new_memfd(n)?),
            Some(n) => w.mem.reserve_exact(n as usize),
            None => {}
        }
        Ok(w)
    }

    pub fn write(&mut self, chunk: &[u8]) -> Result<(), BlobError> {
        self.len += chunk.len() as u64;
        if let Some(extra) = self.len.checked_sub(self.reservation.bytes()) {
            self.reservation.grow(extra)?;
        }
        if let Some(f) = &mut self.file {
            return Ok(f.write_all(chunk)?);
        }
        self.mem.extend_from_slice(chunk);
        if self.mem.len() >= self.threshold {
            let mut f = new_memfd(0)?;
            f.write_all(&self.mem)?;
            self.mem = Vec::new();
            self.file = Some(f);
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<Bytes, BlobError> {
        self.reservation.shrink_to(self.len);
        let Some(f) = self.file else {
            if self.mem.is_empty() {
                return Ok(Bytes::new());
            }
            self.mem.shrink_to_fit();
            return Ok(Bytes::from_owner(Tracked::new(self.mem, self.reservation)));
        };
        if f.metadata()?.len() == 0 {
            return Ok(Bytes::new());
        }
        // SAFETY: the memfd is private to this process and never written again.
        let map = unsafe { memmap2::Mmap::map(&f)? };
        Ok(Bytes::from_owner(Tracked::new(map, self.reservation)))
    }
}

fn new_memfd(size_hint: u64) -> std::io::Result<File> {
    use rustix::fs::{MemfdFlags, memfd_create};
    let fd = memfd_create("fakes3-blob", MemfdFlags::CLOEXEC)?;
    let f = File::from(fd);
    if size_hint > 0 {
        // Reserve pages up front so writes don't grow the file piecemeal.
        let _ = rustix::fs::fallocate(&f, rustix::fs::FallocateFlags::KEEP_SIZE, 0, size_hint);
    }
    Ok(f)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blob(memory: &Arc<MemoryTracker>, threshold: usize, len: usize, hint: bool) -> Result<Bytes, BlobError> {
        let mut w = BlobWriter::new(memory, threshold, hint.then_some(len as u64))?;
        for chunk in vec![5u8; len].chunks(100) {
            w.write(chunk)?;
        }
        w.finish()
    }

    fn object(data: Bytes) -> Object {
        Object {
            data: ObjectData::from_segments(vec![data]),
            meta: ObjectMeta::default(),
            e_tag: String::new(),
            last_modified: SystemTime::UNIX_EPOCH,
            checksum: Default::default(),
            checksum_type: None,
            parts_count: None,
            part_sizes: None,
            tags: Vec::new(),
            ext: None,
            acl: None,
        }
    }

    #[test]
    fn writer_accounts_heap_and_memfd_with_and_without_a_size_hint() {
        let t = MemoryTracker::new(None);
        for hint in [true, false] {
            // 250 bytes with a 1000-byte threshold stays on the heap; 2500 goes to a memfd.
            let small = blob(&t, 1000, 250, hint).unwrap();
            let big = blob(&t, 1000, 2500, hint).unwrap();
            assert_eq!(t.stats().used, 2750, "hint={hint}");
            let slice = big.slice(10..20);
            drop(big);
            assert_eq!(t.stats().used, 2750, "a slice keeps its whole buffer alive");
            drop(slice);
            drop(small);
            assert_eq!(t.stats().used, 0, "hint={hint}");
        }
        assert_eq!(blob(&t, 1000, 0, true).unwrap().len(), 0);
        assert_eq!(blob(&t, 1000, 0, false).unwrap().len(), 0);
        assert_eq!(t.stats().used, 0);
    }

    #[test]
    fn writer_refuses_up_front_or_mid_stream_and_releases() {
        let t = MemoryTracker::new(Some(1000));
        let keep = blob(&t, 4096, 600, true).unwrap();
        assert!(matches!(BlobWriter::new(&t, 4096, Some(500)), Err(BlobError::Limit(_))));
        assert_eq!(t.stats().used, 600);

        // Unknown length: charged as it arrives, and everything is returned when it fails.
        let mut w = BlobWriter::new(&t, 4096, None).unwrap();
        w.write(&[0; 300]).unwrap();
        assert_eq!(t.stats().used, 900);
        assert!(matches!(w.write(&[0; 200]), Err(BlobError::Limit(_))));
        drop(w);
        assert_eq!(t.stats().used, 600);
        drop(keep);
        assert_eq!(t.stats().used, 0);
    }

    #[test]
    fn writer_dropped_unfinished_releases_its_reservation() {
        let t = MemoryTracker::new(Some(10_000));
        let mut w = BlobWriter::new(&t, 100, Some(5000)).unwrap();
        w.write(&[1; 300]).unwrap();
        assert_eq!(t.stats().used, 5000);
        drop(w);
        assert_eq!(t.stats().used, 0);
    }

    #[test]
    fn short_body_gives_back_the_unused_part_of_the_reservation() {
        let t = MemoryTracker::new(None);
        let mut w = BlobWriter::new(&t, 4096, Some(1000)).unwrap();
        w.write(&[1; 400]).unwrap();
        let b = w.finish().unwrap();
        assert_eq!((b.len(), t.stats().used), (400, 400));
    }

    #[test]
    fn dropping_buckets_releases_everything_including_shared_and_versioned_data() {
        let t = MemoryTracker::new(None);
        let store = Store::default();
        let bucket = Arc::new(Bucket::new(SystemTime::UNIX_EPOCH, UserId::DEFAULT));
        *bucket.versioning.write() = Versioning::Enabled;
        store.buckets.write().insert("b".into(), bucket.clone());

        let shared = blob(&t, 1000, 2500, true).unwrap();
        {
            let mut objects = bucket.objects.write();
            bucket.put(&mut objects, "a".into(), object(shared.clone()), UserId::DEFAULT);
            bucket.put(
                &mut objects,
                "a".into(),
                object(blob(&t, 1000, 300, true).unwrap()),
                UserId::DEFAULT,
            );
            bucket.put(&mut objects, "copy".into(), object(shared.slice(0..100)), UserId::DEFAULT);
        }
        bucket.uploads.write().insert(
            ("k".into(), "u".into()),
            Upload {
                initiator: UserId::DEFAULT,
                owner: UserId::DEFAULT,
                meta: ObjectMeta::default(),
                tags: Vec::new(),
                ext: None,
                acl: None,
                initiated: SystemTime::UNIX_EPOCH,
                checksum_algorithm: None,
                checksum_type: s3s::dto::ChecksumType::from_static(s3s::dto::ChecksumType::FULL_OBJECT),
                parts: BTreeMap::from([(
                    1,
                    Part {
                        data: ObjectData::from_segments(vec![blob(&t, 1000, 700, true).unwrap()]),
                        md5: [0; 16],
                        last_modified: SystemTime::UNIX_EPOCH,
                        checksum: Default::default(),
                    },
                )]),
            },
        );
        drop(shared);
        assert_eq!(t.stats().used, 2500 + 300 + 700);

        // One version, then the version deleted by id.
        Bucket::delete_version(&mut bucket.objects.write(), "a", "nonexistent");
        let first = bucket.objects.read()["a"].versions[0].id.clone();
        Bucket::delete_version(&mut bucket.objects.write(), "a", &first);
        assert_eq!(t.stats().used, 2500 + 300 + 700, "'copy' still pins the first buffer");

        // A reset is just dropping the buckets.
        drop(bucket);
        store.buckets.write().clear();
        assert_eq!(t.stats().used, 0);
    }
}
