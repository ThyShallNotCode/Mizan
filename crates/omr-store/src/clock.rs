//! Wall clock and hybrid logical clock for event ordering (spec §14.4, §20.1 "Time").
//!
//! Events are ordered by `seq` and the HLC only; `wall_ts_utc` is informational. The HLC packs
//! milliseconds since the Unix epoch in the high 48 bits and a logical counter in the low 16, so
//! it is strictly increasing even when the wall clock steps back (§22: clock skew).

use std::sync::atomic::{AtomicI64, Ordering};

/// A source of wall-clock time in Unix milliseconds.
pub trait Clock: Send {
    /// Milliseconds since 1970-01-01T00:00:00Z.
    fn now_unix_ms(&self) -> i64;
}

/// The operating-system clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_unix_ms(&self) -> i64 {
        match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => i64::try_from(d.as_millis()).unwrap_or(i64::MAX),
            Err(_) => 0,
        }
    }
}

/// A deterministic clock for tests and replays: starts at `start` and advances by `step` ms on
/// every reading.
#[derive(Debug)]
pub struct StepClock {
    next: AtomicI64,
    step: i64,
}

impl StepClock {
    /// A clock starting at `start_ms`, advancing `step_ms` per reading.
    pub fn new(start_ms: i64, step_ms: i64) -> Self {
        Self {
            next: AtomicI64::new(start_ms),
            step: step_ms,
        }
    }
}

impl Clock for StepClock {
    fn now_unix_ms(&self) -> i64 {
        self.next.fetch_add(self.step, Ordering::Relaxed)
    }
}

/// Next HLC value: `max(now << 16, last + 1)`.
pub fn hlc_next(last: i64, now_ms: i64) -> i64 {
    let physical = now_ms.max(0).saturating_mul(1 << 16);
    physical.max(last.saturating_add(1))
}

/// Physical milliseconds of an HLC value.
pub fn hlc_millis(hlc: i64) -> i64 {
    hlc >> 16
}

/// RFC 3339 UTC timestamp with millisecond precision, e.g. `2026-10-16T08:30:00.000Z`.
pub fn rfc3339_utc(unix_ms: i64) -> String {
    let secs = unix_ms.div_euclid(1000);
    let ms = unix_ms.rem_euclid(1000);
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{ms:03}Z",
        tod / 3600,
        (tod / 60) % 60,
        tod % 60
    )
}

/// Days since 1970-01-01 → (year, month, day), proleptic Gregorian (H. Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_formats() {
        assert_eq!(rfc3339_utc(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(rfc3339_utc(951_782_400_123), "2000-02-29T00:00:00.123Z");
        assert_eq!(rfc3339_utc(1_792_054_800_000), "2026-10-15T09:00:00.000Z");
        assert_eq!(rfc3339_utc(-1), "1969-12-31T23:59:59.999Z");
    }

    #[test]
    fn hlc_is_strictly_increasing_under_clock_steps_back() {
        let a = hlc_next(0, 1_000);
        let b = hlc_next(a, 999); // the wall clock stepped back
        let c = hlc_next(b, 2_000);
        assert!(a < b && b < c);
        assert_eq!(hlc_millis(a), 1_000);
        assert_eq!(hlc_millis(c), 2_000);
    }

    #[test]
    fn step_clock_advances() {
        let c = StepClock::new(10, 5);
        assert_eq!(c.now_unix_ms(), 10);
        assert_eq!(c.now_unix_ms(), 15);
    }
}
