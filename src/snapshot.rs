//! Store-wide snapshots, restore and reset.
//!
//! A snapshot is a copy of every bucket's index, versioning state, tags and in-progress uploads.
//! Object data is never copied: versions are `Arc`s and the bytes behind them are immutable and
//! reference counted, so a snapshot only pins data the live store may since have overwritten.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;

use parking_lot::RwLock;

use crate::store::{Bucket, Index, Store, Upload, Versioning};

pub struct StoreSnapshot {
    buckets: Vec<(String, BucketSnapshot)>,
}

struct BucketSnapshot {
    created: SystemTime,
    versioning: Versioning,
    tags: Option<Vec<(String, String)>>,
    objects: Index,
    uploads: BTreeMap<(String, String), Upload>,
}

/// What a snapshot holds, for listings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub buckets: usize,
    /// Keys with at least one version (delete markers included).
    pub objects: usize,
    /// Multipart uploads in progress.
    pub uploads: usize,
}

impl StoreSnapshot {
    pub fn counts(&self) -> Counts {
        let mut c = Counts {
            buckets: self.buckets.len(),
            ..Counts::default()
        };
        for (_, b) in &self.buckets {
            c.objects += b.objects.len();
            c.uploads += b.uploads.len();
        }
        c
    }
}

impl Store {
    /// Copies the store. Each bucket is copied under its own locks, so a snapshot taken while
    /// requests are in flight is consistent per bucket but not across buckets.
    pub fn snapshot(&self) -> StoreSnapshot {
        let buckets = self.buckets.read();
        let buckets = buckets
            .iter()
            .map(|(name, b)| {
                // Listing every field makes adding one to `Bucket` a compile error here.
                let Bucket {
                    created,
                    versioning,
                    tags,
                    objects,
                    uploads,
                } = &**b;
                let snap = BucketSnapshot {
                    created: *created,
                    versioning: *versioning.read(),
                    tags: tags.read().clone(),
                    objects: objects.read().clone(),
                    uploads: uploads.read().clone(),
                };
                (name.clone(), snap)
            })
            .collect();
        StoreSnapshot { buckets }
    }

    /// Replaces the store's contents with a copy of `snap`; the snapshot stays usable.
    /// Requests already holding a bucket finish against the discarded one.
    pub fn restore(&self, snap: &StoreSnapshot) {
        let buckets = snap
            .buckets
            .iter()
            .map(|(name, s)| {
                let BucketSnapshot {
                    created,
                    versioning,
                    tags,
                    objects,
                    uploads,
                } = s;
                let bucket = Bucket {
                    created: *created,
                    versioning: RwLock::new(*versioning),
                    tags: RwLock::new(tags.clone()),
                    objects: RwLock::new(objects.clone()),
                    uploads: RwLock::new(uploads.clone()),
                };
                (name.clone(), Arc::new(bucket))
            })
            .collect();
        *self.buckets.write() = buckets;
    }

    /// Drops every bucket, object and upload.
    pub fn clear(&self) {
        // Take the old map out first so the (possibly large) drop happens after the lock is released.
        let old = std::mem::take(&mut *self.buckets.write());
        drop(old);
    }

    pub fn counts(&self) -> Counts {
        let buckets = self.buckets.read();
        let mut c = Counts {
            buckets: buckets.len(),
            ..Counts::default()
        };
        for b in buckets.values() {
            c.objects += b.objects.read().len();
            c.uploads += b.uploads.read().len();
        }
        c
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::store::{Entry, Object, ObjectData, Version};

    // The binary uses mimalloc; measure with the same allocator.
    #[global_allocator]
    static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

    fn object(i: usize) -> Object {
        Object {
            data: ObjectData::from_segments(vec![bytes::Bytes::from_static(b"x")]),
            meta: Default::default(),
            e_tag: format!("\"{i}\""),
            last_modified: SystemTime::UNIX_EPOCH,
            checksum: Default::default(),
            checksum_type: None,
            parts_count: None,
            part_sizes: None,
            tags: Vec::new(),
        }
    }

    fn fill(store: &Store, bucket: &str, n: usize) {
        let b = Arc::new(Bucket::new(SystemTime::UNIX_EPOCH));
        {
            let mut objects = b.objects.write();
            for i in 0..n {
                // Same key shape as the benchmarks: a short prefix and a counter.
                b.put(&mut objects, format!("prefix/{i:08}"), object(i));
            }
        }
        store.buckets.write().insert(bucket.to_owned(), b);
    }

    #[test]
    fn snapshot_restore_clear() {
        let store = Store::default();
        fill(&store, "a", 10);
        fill(&store, "b", 5);
        let snap = store.snapshot();
        assert_eq!(
            snap.counts(),
            Counts {
                buckets: 2,
                objects: 15,
                uploads: 0
            }
        );

        // Mutations after the snapshot do not leak into it.
        {
            let a = store.bucket("a").unwrap();
            let mut objects = a.objects.write();
            objects.remove("prefix/00000000");
            objects
                .entry("new".into())
                .or_insert_with(Entry::default)
                .versions
                .push(Arc::new(Version {
                    id: "null".into(),
                    obj: Some(object(99)),
                    last_modified: SystemTime::UNIX_EPOCH,
                }));
        }
        store.buckets.write().remove("b");
        assert_eq!(
            store.counts(),
            Counts {
                buckets: 1,
                objects: 10,
                uploads: 0
            }
        );

        store.restore(&snap);
        assert_eq!(
            store.counts(),
            Counts {
                buckets: 2,
                objects: 15,
                uploads: 0
            }
        );
        assert!(store.bucket("a").unwrap().objects.read().contains_key("prefix/00000000"));
        assert!(!store.bucket("a").unwrap().objects.read().contains_key("new"));

        // Restoring again after further changes works: the snapshot is not consumed.
        store.clear();
        assert_eq!(store.counts(), Counts::default());
        store.restore(&snap);
        assert_eq!(store.counts().objects, 15);
    }

    /// Timing at scale: `cargo test --release snapshot_scale -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn snapshot_scale() {
        for n in [1_000, 100_000, 1_000_000] {
            let store = Store::default();
            let t = Instant::now();
            fill(&store, "bench", n);
            let fill_time = t.elapsed();

            // Best of a few runs: the first one pays for allocator warm-up.
            let mut snapshot = std::time::Duration::MAX;
            let mut restore = std::time::Duration::MAX;
            let mut mutate = std::time::Duration::MAX;
            for _ in 0..5 {
                let t = Instant::now();
                let snap = store.snapshot();
                snapshot = snapshot.min(t.elapsed());

                // Cost of the first writes after a snapshot (copy-on-write for a persistent map).
                let t = Instant::now();
                {
                    let b = store.bucket("bench").unwrap();
                    let mut objects = b.objects.write();
                    for i in 0..1000 {
                        b.put(&mut objects, format!("prefix/{:08}", i * (n / 1000)), object(i));
                    }
                }
                mutate = mutate.min(t.elapsed());

                let t = Instant::now();
                store.restore(&snap);
                restore = restore.min(t.elapsed());
            }
            // Drop of a large store is part of `reset`.
            let t = Instant::now();
            store.clear();
            let clear = t.elapsed();
            eprintln!(
                "snapshot_scale n={n}: fill {fill_time:?}, snapshot {snapshot:?}, 1000 puts after snapshot {mutate:?}, restore {restore:?}, clear {clear:?}, rss {} MiB",
                rss_mib()
            );
        }
    }

    fn rss_mib() -> u64 {
        let statm = std::fs::read_to_string("/proc/self/statm").unwrap_or_default();
        let pages: u64 = statm.split_whitespace().nth(1).and_then(|v| v.parse().ok()).unwrap_or(0);
        pages * 4096 / (1 << 20)
    }
}
