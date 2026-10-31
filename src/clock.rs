//! Server time. Everything that stamps or compares object times reads [`Clock::now`], so the
//! control plane can freeze or move time for one server without touching the host clock.
//! Each server owns its clock: several in-process servers in one test binary stay independent.

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Default)]
pub struct Clock {
    /// Nanoseconds added to the host clock.
    offset: AtomicI64,
    /// When set, `offset` is the absolute time in nanoseconds since the epoch instead.
    frozen: AtomicBool,
}

impl Clock {
    pub fn now(&self) -> SystemTime {
        let off = self.offset.load(Ordering::Relaxed);
        if self.frozen.load(Ordering::Relaxed) {
            return from_nanos(off);
        }
        let host = SystemTime::now();
        if off >= 0 {
            host + Duration::from_nanos(off as u64)
        } else {
            host - Duration::from_nanos(off.unsigned_abs())
        }
    }

    /// Stops time at `at` (or at the current server time).
    pub fn freeze(&self, at: Option<SystemTime>) {
        let at = at.unwrap_or_else(|| self.now());
        self.offset.store(to_nanos(at), Ordering::Relaxed);
        self.frozen.store(true, Ordering::Relaxed);
    }

    /// Lets time run again from the current server time.
    pub fn unfreeze(&self) {
        let now = self.now();
        self.frozen.store(false, Ordering::Relaxed);
        self.offset.store(to_nanos(now) - to_nanos(SystemTime::now()), Ordering::Relaxed);
    }

    /// Jumps to `at`, keeping the frozen/running state.
    pub fn set(&self, at: SystemTime) {
        let target = to_nanos(at);
        if self.frozen.load(Ordering::Relaxed) {
            self.offset.store(target, Ordering::Relaxed);
        } else {
            self.offset.store(target - to_nanos(SystemTime::now()), Ordering::Relaxed);
        }
    }

    /// Moves time forward (or backward, for negative `nanos`).
    pub fn advance(&self, nanos: i64) {
        self.offset.fetch_add(nanos, Ordering::Relaxed);
    }

    /// Back to the host clock.
    pub fn reset(&self) {
        self.frozen.store(false, Ordering::Relaxed);
        self.offset.store(0, Ordering::Relaxed);
    }

    pub fn is_frozen(&self) -> bool {
        self.frozen.load(Ordering::Relaxed)
    }
}

fn to_nanos(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_nanos() as i64,
        Err(e) => -(e.duration().as_nanos() as i64),
    }
}

fn from_nanos(n: i64) -> SystemTime {
    if n >= 0 {
        UNIX_EPOCH + Duration::from_nanos(n as u64)
    } else {
        UNIX_EPOCH - Duration::from_nanos(n.unsigned_abs())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freeze_advance_unfreeze() {
        let c = Clock::default();
        let t = UNIX_EPOCH + Duration::from_secs(1_000_000);
        c.freeze(Some(t));
        assert_eq!(c.now(), t);
        c.advance(5_000_000_000);
        assert_eq!(c.now(), t + Duration::from_secs(5));
        c.unfreeze();
        let after = c.now();
        assert!(after >= t + Duration::from_secs(5) && after < t + Duration::from_secs(6));
        c.reset();
        assert!(c.now().duration_since(SystemTime::now()).unwrap_or_default() < Duration::from_secs(1));
    }
}
