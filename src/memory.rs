//! Memory accounting for object data.
//!
//! Every byte of object data lives in a buffer (a heap `Vec` or a memfd mapping) that is wrapped in
//! a [`Tracked`] owner. The owner holds a [`Reservation`] against the server's [`MemoryTracker`] and
//! gives it back when the buffer is freed, i.e. when the last `Bytes` slice of it goes away. Blobs
//! are shared between objects, versions, copies and multipart parts, so each buffer is counted
//! exactly once and released exactly when nothing references it any more, whatever dropped the
//! last reference (overwrite, delete, abort, a finished GET stream, a reset).
//!
//! The hot path is a handful of relaxed/CAS atomics on one cache line: no locks. The count is the
//! size of the data buffers only (not metadata, keys, or allocator overhead).

use std::fmt;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Warn when usage reaches this share of the limit, and re-arm once it falls below `REARM`.
const WARN_PERCENT: u64 = 90;
const REARM_PERCENT: u64 = 80;

/// The share of the available memory used when the limit is not set explicitly.
const AUTO_PERCENT: u64 = 75;

/// How the memory limit is chosen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MemoryLimit {
    /// 75% of the container's cgroup memory limit or the machine's physical memory, whichever is
    /// smaller; unlimited if neither can be determined.
    #[default]
    Auto,
    Unlimited,
    /// A fixed number of bytes.
    Bytes(u64),
}

impl MemoryLimit {
    /// The limit in bytes, or `None` for unlimited.
    pub fn resolve(self) -> Option<u64> {
        match self {
            Self::Auto => available_memory().map(|m| m / 100 * AUTO_PERCENT),
            Self::Unlimited => None,
            Self::Bytes(n) => Some(n),
        }
    }
}

impl FromStr for MemoryLimit {
    type Err = String;

    /// Accepts `auto`, `unlimited` (or `none`, `0`), or a size such as `512MiB`, `2GiB`, `1.5GB`,
    /// `4096` (bytes). `KB`/`MB`/`GB`/`TB` are powers of 1000, `KiB`/`MiB`/`GiB`/`TiB` of 1024;
    /// a bare `K`/`M`/`G`/`T` suffix means the 1024 variants.
    fn from_str(s: &str) -> Result<Self, String> {
        let t = s.trim();
        match t.to_ascii_lowercase().as_str() {
            "auto" => return Ok(Self::Auto),
            "unlimited" | "none" | "off" => return Ok(Self::Unlimited),
            _ => {}
        }
        let split = t.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(t.len());
        let (num, unit) = t.split_at(split);
        let num: f64 = num
            .parse()
            .map_err(|_| format!("invalid memory limit {s:?}: expected a size like 512MiB or 2GiB"))?;
        let mult: f64 = match unit.trim().to_ascii_lowercase().as_str() {
            "" | "b" => 1.0,
            "k" | "kib" => 1024.0,
            "m" | "mib" => 1024.0 * 1024.0,
            "g" | "gib" => 1024.0 * 1024.0 * 1024.0,
            "t" | "tib" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
            "kb" => 1e3,
            "mb" => 1e6,
            "gb" => 1e9,
            "tb" => 1e12,
            _ => return Err(format!("invalid memory limit {s:?}: unknown unit {unit:?}")),
        };
        let bytes = num * mult;
        if !bytes.is_finite() || bytes >= u64::MAX as f64 {
            return Err(format!("invalid memory limit {s:?}: too large"));
        }
        Ok(if bytes < 1.0 { Self::Unlimited } else { Self::Bytes(bytes as u64) })
    }
}

/// Physical memory, capped by the cgroup limit when running in a container.
fn available_memory() -> Option<u64> {
    let physical = std::fs::read_to_string("/proc/meminfo").ok().and_then(|s| {
        let line = s.lines().find(|l| l.starts_with("MemTotal:"))?;
        let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
        kb.checked_mul(1024)
    });
    // cgroup v2, then v1. "max" (v2) or a value near i64::MAX (v1) means no limit.
    let cgroup = ["/sys/fs/cgroup/memory.max", "/sys/fs/cgroup/memory/memory.limit_in_bytes"]
        .iter()
        .find_map(|p| std::fs::read_to_string(p).ok()?.trim().parse::<u64>().ok());
    match (physical, cgroup) {
        (Some(p), Some(c)) => Some(p.min(c)),
        (p, c) => p.or(c),
    }
}

/// A point-in-time reading of the memory accounting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryStats {
    /// Bytes of object data currently held (including uploaded multipart parts and data kept alive
    /// only by an in-flight read).
    pub used: u64,
    /// The configured limit, `None` if unlimited.
    pub limit: Option<u64>,
}

/// The write was refused because it would exceed the memory limit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimitExceeded {
    pub used: u64,
    pub limit: u64,
    /// Bytes the rejected request asked for (or had buffered when it crossed the limit).
    pub requested: u64,
}

impl fmt::Display for LimitExceeded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "The fakes3 memory limit was reached: it holds {} of its {} limit and this request needs {} more. \
             Raise the limit with --memory-limit (env FAKES3_MEMORY_LIMIT, or Config::memory_limit when embedded), \
             e.g. --memory-limit 8GiB, or use --memory-limit unlimited. Deleting objects or aborting multipart uploads frees memory.",
            human(self.used),
            human(self.limit),
            human(self.requested),
        )
    }
}

impl std::error::Error for LimitExceeded {}

/// Formats a byte count with a binary unit, e.g. `1.5 GiB`.
pub fn human(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i + 1 < UNITS.len() {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{n} B") } else { format!("{v:.1} {}", UNITS[i]) }
}

/// Counts the bytes of object data held by one server.
pub struct MemoryTracker {
    used: AtomicU64,
    /// `u64::MAX` when unlimited.
    limit: u64,
    warn_at: u64,
    rearm_below: u64,
    warned: AtomicBool,
}

impl MemoryTracker {
    pub fn new(limit: Option<u64>) -> Arc<Self> {
        let limit_v = limit.unwrap_or(u64::MAX);
        Arc::new(Self {
            used: AtomicU64::new(0),
            limit: limit_v,
            warn_at: percent_of(limit_v, WARN_PERCENT),
            rearm_below: percent_of(limit_v, REARM_PERCENT),
            warned: AtomicBool::new(false),
        })
    }

    pub fn stats(&self) -> MemoryStats {
        MemoryStats {
            used: self.used.load(Ordering::Relaxed),
            limit: (self.limit != u64::MAX).then_some(self.limit),
        }
    }

    /// Reserves `n` bytes, or fails if that would pass the limit.
    pub fn reserve(self: &Arc<Self>, n: u64) -> Result<Reservation, LimitExceeded> {
        let mut r = Reservation {
            tracker: self.clone(),
            bytes: 0,
        };
        r.grow(n)?;
        Ok(r)
    }

    fn add(&self, n: u64) -> Result<(), LimitExceeded> {
        if n == 0 {
            return Ok(());
        }
        if self.limit == u64::MAX {
            self.used.fetch_add(n, Ordering::Relaxed);
            return Ok(());
        }
        let mut cur = self.used.load(Ordering::Relaxed);
        loop {
            let next = match cur.checked_add(n) {
                Some(next) if next <= self.limit => next,
                _ => {
                    return Err(LimitExceeded {
                        used: cur,
                        limit: self.limit,
                        requested: n,
                    });
                }
            };
            match self.used.compare_exchange_weak(cur, next, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => {
                    if next >= self.warn_at && !self.warned.swap(true, Ordering::Relaxed) {
                        tracing::warn!(
                            "fakes3 is holding {} of its {} memory limit; writes will be rejected once it is full \
                             (raise it with --memory-limit / FAKES3_MEMORY_LIMIT)",
                            human(next),
                            human(self.limit)
                        );
                    }
                    return Ok(());
                }
                Err(actual) => cur = actual,
            }
        }
    }

    fn sub(&self, n: u64) {
        if n == 0 {
            return;
        }
        let prev = self.used.fetch_sub(n, Ordering::Relaxed);
        debug_assert!(prev >= n, "memory accounting underflow");
        if prev - n < self.rearm_below && self.warned.load(Ordering::Relaxed) {
            self.warned.store(false, Ordering::Relaxed);
        }
    }
}

fn percent_of(n: u64, pct: u64) -> u64 {
    if n == u64::MAX { u64::MAX } else { n / 100 * pct }
}

/// Bytes charged to a tracker, returned on drop.
pub struct Reservation {
    tracker: Arc<MemoryTracker>,
    bytes: u64,
}

impl Reservation {
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Charges `n` more bytes.
    pub fn grow(&mut self, n: u64) -> Result<(), LimitExceeded> {
        self.tracker.add(n)?;
        self.bytes += n;
        Ok(())
    }

    /// Gives back everything above `n` bytes.
    pub fn shrink_to(&mut self, n: u64) {
        if let Some(extra) = self.bytes.checked_sub(n) {
            self.tracker.sub(extra);
            self.bytes = n;
        }
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.tracker.sub(self.bytes);
    }
}

/// A buffer together with the reservation that pays for it. Pass this to `Bytes::from_owner` so
/// the memory is released when the last `Bytes` referencing the buffer is dropped.
pub struct Tracked<T> {
    buf: T,
    _reservation: Reservation,
}

impl<T> Tracked<T> {
    pub fn new(buf: T, reservation: Reservation) -> Self {
        Self {
            buf,
            _reservation: reservation,
        }
    }
}

impl<T: AsRef<[u8]>> AsRef<[u8]> for Tracked<T> {
    fn as_ref(&self) -> &[u8] {
        self.buf.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn used(t: &MemoryTracker) -> u64 {
        t.stats().used
    }

    #[test]
    fn reserve_and_release() {
        let t = MemoryTracker::new(Some(100));
        let a = t.reserve(60).unwrap();
        assert_eq!(used(&t), 60);
        let err = t.reserve(41).err().unwrap();
        assert_eq!((err.used, err.limit, err.requested), (60, 100, 41));
        assert_eq!(used(&t), 60, "a failed reservation leaves no trace");
        let b = t.reserve(40).unwrap();
        assert_eq!(used(&t), 100);
        drop(a);
        assert_eq!(used(&t), 40);
        drop(b);
        assert_eq!(used(&t), 0);
    }

    #[test]
    fn grow_shrink_and_drop_release_everything() {
        let t = MemoryTracker::new(Some(100));
        let mut r = t.reserve(10).unwrap();
        r.grow(50).unwrap();
        assert!(r.grow(41).is_err());
        assert_eq!((r.bytes(), used(&t)), (60, 60));
        r.shrink_to(20);
        assert_eq!((r.bytes(), used(&t)), (20, 20));
        r.shrink_to(30);
        assert_eq!(used(&t), 20, "shrink_to never grows");
        drop(r);
        assert_eq!(used(&t), 0);
    }

    #[test]
    fn unlimited_never_refuses() {
        let t = MemoryTracker::new(None);
        let r = t.reserve(u64::MAX / 2).unwrap();
        assert_eq!(t.stats().limit, None);
        drop(r);
        assert_eq!(used(&t), 0);
    }

    #[test]
    fn overflowing_request_is_refused() {
        let t = MemoryTracker::new(Some(100));
        let _a = t.reserve(10).unwrap();
        assert!(t.reserve(u64::MAX).is_err());
        assert_eq!(used(&t), 10);
    }

    #[test]
    fn tracked_owner_releases_with_last_slice() {
        let t = MemoryTracker::new(Some(1000));
        let r = t.reserve(8).unwrap();
        let b = bytes::Bytes::from_owner(Tracked::new(vec![1u8; 8], r));
        let s1 = b.slice(0..4);
        let s2 = b.slice(4..8);
        drop(b);
        assert_eq!(used(&t), 8);
        drop(s1);
        assert_eq!(used(&t), 8);
        drop(s2);
        assert_eq!(used(&t), 0);
    }

    #[test]
    fn warning_rearms_after_usage_drops() {
        let t = MemoryTracker::new(Some(100));
        let a = t.reserve(95).unwrap();
        assert!(t.warned.load(Ordering::Relaxed));
        let _b = t.reserve(1).unwrap();
        drop(a);
        assert!(!t.warned.load(Ordering::Relaxed));
    }

    #[test]
    fn concurrent_reservations_never_pass_the_limit() {
        let t = MemoryTracker::new(Some(1000));
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let t = t.clone();
                std::thread::spawn(move || {
                    let mut held = Vec::new();
                    for _ in 0..1000 {
                        if let Ok(r) = t.reserve(7) {
                            assert!(t.stats().used <= 1000);
                            held.push(r);
                        }
                        if held.len() > 10 {
                            held.clear();
                        }
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(used(&t), 0);
    }

    #[test]
    fn parses_limits() {
        let p = |s: &str| s.parse::<MemoryLimit>();
        assert_eq!(p("auto"), Ok(MemoryLimit::Auto));
        assert_eq!(p("Unlimited"), Ok(MemoryLimit::Unlimited));
        assert_eq!(p("0"), Ok(MemoryLimit::Unlimited));
        assert_eq!(p("4096"), Ok(MemoryLimit::Bytes(4096)));
        assert_eq!(p("512MiB"), Ok(MemoryLimit::Bytes(512 << 20)));
        assert_eq!(p("2g"), Ok(MemoryLimit::Bytes(2 << 30)));
        assert_eq!(p("1.5 GiB"), Ok(MemoryLimit::Bytes(3 << 29)));
        assert_eq!(p("3GB"), Ok(MemoryLimit::Bytes(3_000_000_000)));
        assert!(p("lots").is_err());
        assert!(p("12 parsecs").is_err());
        assert!(p("").is_err());
    }

    #[test]
    fn message_names_the_limit_and_the_flag() {
        let msg = LimitExceeded {
            used: 3 << 30,
            limit: 4 << 30,
            requested: 2 << 30,
        }
        .to_string();
        assert!(msg.contains("fakes3 memory limit"), "{msg}");
        assert!(msg.contains("--memory-limit") && msg.contains("FAKES3_MEMORY_LIMIT"), "{msg}");
        assert!(msg.contains("3.0 GiB") && msg.contains("4.0 GiB"), "{msg}");
    }
}
