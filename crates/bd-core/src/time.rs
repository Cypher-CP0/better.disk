//! Timestamps as Windows FILETIME ticks (100 ns intervals since
//! 1601-01-01 UTC), the representation every Windows metadata API returns.

use std::time::{SystemTime, UNIX_EPOCH};

pub const TICKS_PER_SECOND: i64 = 10_000_000;
pub const TICKS_PER_DAY: i64 = 86_400 * TICKS_PER_SECOND;
/// FILETIME of 1970-01-01T00:00:00Z.
pub const UNIX_EPOCH_TICKS: i64 = 116_444_736_000_000_000;
/// FILETIME of 10000-01-01T00:00:00Z. Anything at or beyond is invalid for
/// display and bucketing.
const MAX_VALID_TICKS: i64 = 2_650_467_744_000_000_000;

/// A point in time in FILETIME ticks. Only constructed for valid values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Ticks(i64);

impl Ticks {
    /// Accepts a raw FILETIME. Zero (the "no timestamp" value), negative
    /// values and values past year 9999 are rejected.
    pub fn from_filetime(raw: i64) -> Option<Ticks> {
        (raw > 0 && raw < MAX_VALID_TICKS).then_some(Ticks(raw))
    }

    pub fn raw(self) -> i64 {
        self.0
    }

    pub fn from_system_time(t: SystemTime) -> Option<Ticks> {
        let ticks = match t.duration_since(UNIX_EPOCH) {
            Ok(after) => {
                let t = i64::try_from(after.as_nanos() / 100).ok()?;
                UNIX_EPOCH_TICKS.checked_add(t)?
            }
            Err(before) => {
                let t = i64::try_from(before.duration().as_nanos() / 100).ok()?;
                UNIX_EPOCH_TICKS.checked_sub(t)?
            }
        };
        Ticks::from_filetime(ticks)
    }

    pub fn now() -> Ticks {
        Ticks::from_system_time(SystemTime::now()).expect("system clock is within 1601..9999")
    }

    /// Ticks elapsed from `self` to `later`; negative when `later` is earlier.
    pub fn elapsed_until(self, later: Ticks) -> i64 {
        later.0 - self.0
    }

    pub fn checked_add_days(self, days: i64) -> Option<Ticks> {
        Ticks::from_filetime(self.0.checked_add(days.checked_mul(TICKS_PER_DAY)?)?)
    }

    pub fn checked_add_ticks(self, ticks: i64) -> Option<Ticks> {
        Ticks::from_filetime(self.0.checked_add(ticks)?)
    }

    /// UTC ISO 8601 with millisecond precision, e.g. `2026-10-09T16:33:00.000Z`.
    pub fn to_iso8601(self) -> String {
        let since_unix = self.0 - UNIX_EPOCH_TICKS;
        let total_ms = since_unix.div_euclid(10_000);
        let days = total_ms.div_euclid(86_400_000);
        let ms_of_day = total_ms.rem_euclid(86_400_000);
        let (y, m, d) = civil_from_days(days);
        let h = ms_of_day / 3_600_000;
        let min = (ms_of_day / 60_000) % 60;
        let s = (ms_of_day / 1000) % 60;
        let ms = ms_of_day % 1000;
        format!("{y:04}-{m:02}-{d:02}T{h:02}:{min:02}:{s:02}.{ms:03}Z")
    }
}

/// Days since 1970-01-01 to a proleptic Gregorian (year, month, day).
/// Howard Hinnant's `civil_from_days`.
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
    use std::time::Duration;

    #[test]
    fn rejects_zero_negative_and_far_future() {
        assert_eq!(Ticks::from_filetime(0), None);
        assert_eq!(Ticks::from_filetime(-5), None);
        assert_eq!(Ticks::from_filetime(MAX_VALID_TICKS), None);
        assert!(Ticks::from_filetime(MAX_VALID_TICKS - 1).is_some());
    }

    #[test]
    fn unix_epoch_round_trip() {
        let t = Ticks::from_system_time(UNIX_EPOCH).unwrap();
        assert_eq!(t.raw(), UNIX_EPOCH_TICKS);
        assert_eq!(t.to_iso8601(), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn iso_formatting_known_dates() {
        // 2026-10-09T16:33:12.345Z
        let secs: u64 = 1_791_563_592;
        let t =
            Ticks::from_system_time(UNIX_EPOCH + Duration::from_millis(secs * 1000 + 345)).unwrap();
        assert_eq!(t.to_iso8601(), "2026-10-09T16:33:12.345Z");
        // Leap day.
        let leap = Ticks::from_system_time(UNIX_EPOCH + Duration::from_secs(951_782_400)).unwrap();
        assert_eq!(leap.to_iso8601(), "2000-02-29T00:00:00.000Z");
        // Before the Unix epoch.
        let early = Ticks::from_filetime(1).unwrap();
        assert_eq!(early.to_iso8601(), "1601-01-01T00:00:00.000Z");
    }

    #[test]
    fn day_arithmetic() {
        let t = Ticks::from_filetime(UNIX_EPOCH_TICKS).unwrap();
        let later = t.checked_add_days(7).unwrap();
        assert_eq!(t.elapsed_until(later), 7 * TICKS_PER_DAY);
        assert_eq!(later.elapsed_until(t), -7 * TICKS_PER_DAY);
    }
}
