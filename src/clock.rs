//! Server time. Everything that stamps or compares object times reads [`Clock::now`], so the
//! control plane can freeze or move time for one server without touching the host clock.
//! Each server owns its clock: several in-process servers in one test binary stay independent.
//!
//! The whole state lives in one atomic word, so `now()` is a single load and every change is an
//! atomic read-modify-write: a reader never sees a half-applied freeze, and concurrent
//! `advance` calls add up instead of overwriting each other.

use std::fmt;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// `value << 1 | frozen`. Running: `value` is the nanoseconds added to the host clock.
/// Frozen: `value` is the absolute time in nanoseconds since the epoch. Both fit in 63 bits,
/// which covers ±146 years (the years 1824 to 2116 when frozen).
#[derive(Default)]
pub struct Clock {
    state: AtomicI64,
}

/// A requested time, or offset, does not fit the clock's range of about ±146 years.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutOfRange;

impl fmt::Display for OutOfRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("time is out of the clock's range (years 1824 to 2116)")
    }
}

impl std::error::Error for OutOfRange {}

fn pack(value: i64, frozen: bool) -> Result<i64, OutOfRange> {
    Ok(value.checked_mul(2).ok_or(OutOfRange)? | frozen as i64)
}

fn unpack(state: i64) -> (i64, bool) {
    (state >> 1, state & 1 == 1)
}

impl Clock {
    pub fn now(&self) -> SystemTime {
        let (value, frozen) = unpack(self.state.load(Ordering::Relaxed));
        if frozen {
            return from_nanos(value);
        }
        let host = SystemTime::now();
        if value >= 0 {
            host + Duration::from_nanos(value as u64)
        } else {
            host - Duration::from_nanos(value.unsigned_abs())
        }
    }

    /// Stops time at `at` (or at the current server time).
    pub fn freeze(&self, at: Option<SystemTime>) -> Result<(), OutOfRange> {
        let at = at.map(to_nanos);
        self.update(|value, frozen| {
            let at = at.unwrap_or(if frozen { value } else { host_nanos() + value });
            (at, true)
        })
    }

    /// Lets time run again from the current server time.
    pub fn unfreeze(&self) -> Result<(), OutOfRange> {
        self.update(|value, frozen| if frozen { (value - host_nanos(), false) } else { (value, false) })
    }

    /// Jumps to `at`, keeping the frozen/running state.
    pub fn set(&self, at: SystemTime) -> Result<(), OutOfRange> {
        let at = to_nanos(at);
        self.update(|_, frozen| (if frozen { at } else { at - host_nanos() }, frozen))
    }

    /// Moves time forward (or backward, for negative `nanos`).
    pub fn advance(&self, nanos: i64) -> Result<(), OutOfRange> {
        self.update(|value, frozen| (value.saturating_add(nanos), frozen))
    }

    /// Back to the host clock.
    pub fn reset(&self) {
        self.state.store(0, Ordering::Relaxed);
    }

    pub fn is_frozen(&self) -> bool {
        unpack(self.state.load(Ordering::Relaxed)).1
    }

    /// Whether the clock differs from the host clock (frozen, or running with an offset).
    pub fn is_virtual(&self) -> bool {
        self.state.load(Ordering::Relaxed) != 0
    }

    fn update(&self, f: impl Fn(i64, bool) -> (i64, bool)) -> Result<(), OutOfRange> {
        // A plain CAS loop: `fetch_update` is deprecated on newer toolchains and its replacement
        // (`try_update`) does not exist on older ones.
        let mut state = self.state.load(Ordering::Relaxed);
        loop {
            let (value, frozen) = unpack(state);
            let (value, frozen) = f(value, frozen);
            let next = pack(value, frozen)?;
            match self.state.compare_exchange_weak(state, next, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => return Ok(()),
                Err(current) => state = current,
            }
        }
    }
}

fn host_nanos() -> i64 {
    to_nanos(SystemTime::now())
}

fn to_nanos(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_nanos()).unwrap_or(i64::MAX),
        Err(e) => i64::try_from(e.duration().as_nanos()).map_or(i64::MIN, |n| -n),
    }
}

fn from_nanos(n: i64) -> SystemTime {
    if n >= 0 {
        UNIX_EPOCH + Duration::from_nanos(n as u64)
    } else {
        UNIX_EPOCH - Duration::from_nanos(n.unsigned_abs())
    }
}

/// Parses a duration such as `30d`, `1h30m`, `500ms` or `-2w`. Units: `ns`, `us`, `ms`, `s`, `m`,
/// `h`, `d` (24 hours), `w`. A bare number is seconds. Returns signed nanoseconds.
pub fn parse_duration(s: &str) -> Result<i64, String> {
    let bad = || format!("invalid duration {s:?} (examples: 30d, 1h30m, 500ms, -2w)");
    let (neg, mut rest) = match s.trim().strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s.trim().trim_start_matches('+')),
    };
    if rest.is_empty() {
        return Err(bad());
    }
    let mut total: i128 = 0;
    while !rest.is_empty() {
        let digits = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
        let n: i128 = rest[..digits].parse().map_err(|_| bad())?;
        rest = &rest[digits..];
        let unit_len = rest.find(|c: char| c.is_ascii_digit()).unwrap_or(rest.len());
        let per: i128 = match &rest[..unit_len] {
            "ns" => 1,
            "us" | "µs" => 1_000,
            "ms" => 1_000_000,
            "" | "s" => 1_000_000_000,
            "m" => 60 * 1_000_000_000,
            "h" => 3600 * 1_000_000_000,
            "d" => 86_400 * 1_000_000_000,
            "w" => 7 * 86_400 * 1_000_000_000,
            _ => return Err(bad()),
        };
        rest = &rest[unit_len..];
        total = total.saturating_add(n.saturating_mul(per));
    }
    let total = if neg { -total } else { total };
    i64::try_from(total).map_err(|_| bad())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freeze_advance_unfreeze() {
        let c = Clock::default();
        let t = UNIX_EPOCH + Duration::from_secs(1_000_000);
        c.freeze(Some(t)).unwrap();
        assert_eq!(c.now(), t);
        c.advance(5_000_000_000).unwrap();
        assert_eq!(c.now(), t + Duration::from_secs(5));
        c.unfreeze().unwrap();
        let after = c.now();
        assert!(after >= t + Duration::from_secs(5) && after < t + Duration::from_secs(6));
        c.reset();
        assert!(!c.is_virtual());
        assert!(c.now().duration_since(SystemTime::now()).unwrap_or_default() < Duration::from_secs(1));
    }

    #[test]
    fn set_keeps_mode() {
        let c = Clock::default();
        let t = UNIX_EPOCH + Duration::from_secs(2_000_000_000);
        c.set(t).unwrap();
        assert!(!c.is_frozen());
        let now = c.now();
        assert!(now >= t && now < t + Duration::from_secs(1));
        c.freeze(None).unwrap();
        let frozen = c.now();
        assert_eq!(c.now(), frozen);
        c.set(t).unwrap();
        assert_eq!(c.now(), t);
    }

    #[test]
    fn advance_backward_and_range() {
        let c = Clock::default();
        c.advance(-3_600_000_000_000).unwrap();
        assert!(c.now() < SystemTime::now() - Duration::from_secs(3590));
        assert_eq!(c.advance(i64::MAX), Err(OutOfRange));
        let far = UNIX_EPOCH + Duration::from_secs(10_000_000_000);
        assert_eq!(c.freeze(Some(far)), Err(OutOfRange));
        // A failed change leaves the state alone.
        assert!(c.now() < SystemTime::now());
    }

    #[test]
    fn concurrent_advances_add_up() {
        let c = std::sync::Arc::new(Clock::default());
        c.freeze(Some(UNIX_EPOCH)).unwrap();
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let c = c.clone();
                std::thread::spawn(move || (0..1000).for_each(|_| c.advance(1_000).unwrap()))
            })
            .collect();
        threads.into_iter().for_each(|t| t.join().unwrap());
        assert_eq!(c.now(), UNIX_EPOCH + Duration::from_micros(8000));
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("30d"), Ok(30 * 86_400 * 1_000_000_000));
        assert_eq!(parse_duration("1h30m"), Ok(5400 * 1_000_000_000));
        assert_eq!(parse_duration("-500ms"), Ok(-500_000_000));
        assert_eq!(parse_duration("2w"), Ok(14 * 86_400 * 1_000_000_000));
        assert_eq!(parse_duration("90"), Ok(90 * 1_000_000_000));
        assert!(parse_duration("").is_err());
        assert!(parse_duration("d").is_err());
        assert!(parse_duration("5x").is_err());
        assert!(parse_duration("99999999999999999999d").is_err());
    }
}
