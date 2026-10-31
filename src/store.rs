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

use bytes::{Bytes, BytesMut};
use parking_lot::RwLock;

use crate::objext::{BucketCfg, ObjectExt};

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
}

/// One entry in a key's version chain. `obj == None` is a delete marker.
pub struct Version {
    pub id: String,
    pub obj: Option<Object>,
    pub last_modified: SystemTime,
}

/// All versions of a key, oldest first. Unversioned buckets keep a single `null` version.
#[derive(Default)]
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

pub struct Part {
    pub data: ObjectData,
    pub md5: [u8; 16],
    pub last_modified: SystemTime,
    pub checksum: s3s::dto::Checksum,
}

pub struct Upload {
    pub meta: ObjectMeta,
    pub tags: Vec<(String, String)>,
    pub ext: Option<ObjectExt>,
    pub initiated: SystemTime,
    pub checksum_algorithm: Option<s3s::dto::ChecksumAlgorithm>,
    /// Effective checksum type (explicit, or the default for the algorithm).
    pub checksum_type: s3s::dto::ChecksumType,
    pub parts: BTreeMap<i32, Part>,
}

pub struct Bucket {
    pub created: SystemTime,
    pub versioning: RwLock<Versioning>,
    pub tags: RwLock<Option<Vec<(String, String)>>>,
    pub objects: RwLock<BTreeMap<String, Entry>>,
    pub uploads: RwLock<BTreeMap<(String, String), Upload>>,
    pub cfg: BucketCfg,
}

#[derive(Default)]
pub struct Store {
    pub buckets: RwLock<BTreeMap<String, Arc<Bucket>>>,
}

impl Store {
    pub fn bucket(&self, name: &str) -> Option<Arc<Bucket>> {
        self.buckets.read().get(name).cloned()
    }
}

impl Bucket {
    pub fn new(now: SystemTime) -> Self {
        Self {
            created: now,
            versioning: RwLock::new(Versioning::Unversioned),
            tags: RwLock::default(),
            objects: RwLock::default(),
            uploads: RwLock::default(),
            cfg: BucketCfg::default(),
        }
    }
}

pub fn new_version_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

impl Bucket {
    /// Stores `obj` as the newest version of `key` and returns the version id to report, if any.
    pub fn put(&self, objects: &mut BTreeMap<String, Entry>, key: String, obj: Object) -> Option<String> {
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
        }));
        (mode != Versioning::Unversioned).then_some(id)
    }

    /// A DELETE without a version id. Returns the id of the delete marker created, if any.
    pub fn delete_current(&self, objects: &mut BTreeMap<String, Entry>, key: &str, now: SystemTime) -> Option<String> {
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
        }));
        Some(id)
    }

    /// Permanently removes one version. Returns whether it was a delete marker, or `None` if absent.
    pub fn delete_version(objects: &mut BTreeMap<String, Entry>, key: &str, id: &str) -> Option<bool> {
        let entry = objects.get_mut(key)?;
        let pos = entry.versions.iter().position(|v| v.id == id)?;
        let removed = entry.versions.remove(pos);
        if entry.versions.is_empty() {
            objects.remove(key);
        }
        Some(removed.obj.is_none())
    }
}

/// Accumulates an incoming body, spilling to a memfd once it reaches `threshold` bytes.
///
/// Small bodies are copied into an exact-size heap buffer rather than keeping the request's
/// chunks, which would pin hyper's (much larger) read buffers.
pub struct BlobWriter {
    threshold: usize,
    mem: BytesMut,
    file: Option<File>,
}

impl BlobWriter {
    pub fn new(threshold: usize, size_hint: Option<u64>) -> std::io::Result<Self> {
        let mut w = Self {
            threshold,
            mem: BytesMut::new(),
            file: None,
        };
        match size_hint {
            Some(n) if n as usize >= threshold => w.file = Some(new_memfd(n)?),
            Some(n) => w.mem.reserve(n as usize),
            None => {}
        }
        Ok(w)
    }

    pub fn write(&mut self, chunk: &[u8]) -> std::io::Result<()> {
        if let Some(f) = &mut self.file {
            return f.write_all(chunk);
        }
        self.mem.extend_from_slice(chunk);
        if self.mem.len() >= self.threshold {
            let mut f = new_memfd(0)?;
            f.write_all(&self.mem)?;
            self.mem = BytesMut::new();
            self.file = Some(f);
        }
        Ok(())
    }

    pub fn finish(self) -> std::io::Result<Bytes> {
        let Some(f) = self.file else { return Ok(self.mem.freeze()) };
        if f.metadata()?.len() == 0 {
            return Ok(Bytes::new());
        }
        // SAFETY: the memfd is private to this process and never written again.
        let map = unsafe { memmap2::Mmap::map(&f)? };
        Ok(Bytes::from_owner(map))
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
