//! Date buckets for treemap shading (FR06).
//!
//! Age is measured from the scan's start time (saved with the snapshot) so
//! a saved scan never silently changes colour as time passes. Boundaries are
//! elapsed 24-hour days with inclusive upper bounds: exactly 7 days old is
//! "Within 7 days", one tick more is "8 to 30 days".

use crate::time::{TICKS_PER_DAY, Ticks};

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase")]
pub enum AgeBucket {
    Within7Days = 0,
    Days8To30 = 1,
    Days31To90 = 2,
    Days91To365 = 3,
    OverOneYear = 4,
    /// Missing, invalid, future, or (by default) unverified access dates.
    Unknown = 5,
}

impl AgeBucket {
    pub const ALL: [AgeBucket; 6] = [
        AgeBucket::Within7Days,
        AgeBucket::Days8To30,
        AgeBucket::Days31To90,
        AgeBucket::Days91To365,
        AgeBucket::OverOneYear,
        AgeBucket::Unknown,
    ];

    /// Known buckets from most to least recent.
    pub const KNOWN: [AgeBucket; 5] = [
        AgeBucket::Within7Days,
        AgeBucket::Days8To30,
        AgeBucket::Days31To90,
        AgeBucket::Days91To365,
        AgeBucket::OverOneYear,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    /// Legend label from the design document.
    pub fn label(self) -> &'static str {
        match self {
            AgeBucket::Within7Days => "Within 7 days",
            AgeBucket::Days8To30 => "8 to 30 days",
            AgeBucket::Days31To90 => "31 to 90 days",
            AgeBucket::Days91To365 => "91 to 365 days",
            AgeBucket::OverOneYear => "Over 1 year",
            AgeBucket::Unknown => "Unknown",
        }
    }
}

/// Bucket for a timestamp relative to the scan reference time.
pub fn bucket_for(reference: Ticks, timestamp: Option<Ticks>) -> AgeBucket {
    let Some(ts) = timestamp else {
        return AgeBucket::Unknown;
    };
    let age = ts.elapsed_until(reference);
    if age < 0 {
        return AgeBucket::Unknown;
    }
    let day = TICKS_PER_DAY;
    if age <= 7 * day {
        AgeBucket::Within7Days
    } else if age <= 30 * day {
        AgeBucket::Days8To30
    } else if age <= 90 * day {
        AgeBucket::Days31To90
    } else if age <= 365 * day {
        AgeBucket::Days91To365
    } else {
        AgeBucket::OverOneYear
    }
}

/// Which timestamp drives shading.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DateMetric {
    #[default]
    Modified,
    Accessed,
}

/// Whether last-access times on a volume can be trusted (FR06). Determined
/// by reading, never changing, the filesystem's access-time policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AccessReliability {
    Known,
    #[default]
    Unverified,
    Unavailable,
}

/// How access dates are treated for the current view.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AccessPolicy {
    pub reliability: AccessReliability,
    /// The user explicitly chose to include unverified access dates; the UI
    /// must show a persistent notice while this is on.
    pub include_unverified: bool,
}

impl AccessPolicy {
    pub fn bucket(self, reference: Ticks, accessed: Option<Ticks>) -> AgeBucket {
        match self.reliability {
            AccessReliability::Known => bucket_for(reference, accessed),
            AccessReliability::Unverified if self.include_unverified => {
                bucket_for(reference, accessed)
            }
            AccessReliability::Unverified | AccessReliability::Unavailable => AgeBucket::Unknown,
        }
    }
}

/// Counted bytes per bucket for an aggregate block.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct AgeHistogram(pub [u128; 6]);

impl AgeHistogram {
    pub fn add(&mut self, bucket: AgeBucket, bytes: u128) {
        self.0[bucket.index()] += bytes;
    }

    pub fn merge(&mut self, other: &AgeHistogram) {
        for (a, b) in self.0.iter_mut().zip(other.0.iter()) {
            *a += *b;
        }
    }

    pub fn get(&self, bucket: AgeBucket) -> u128 {
        self.0[bucket.index()]
    }

    /// The bucket a block is shaded with (FR06):
    ///
    /// * the bucket holding the most counted bytes;
    /// * ties between known buckets go to the most recent one;
    /// * Unknown competes as a bucket and wins when strictly largest;
    /// * a tie involving Unknown goes to Unknown (conservative);
    /// * a block with no counted bytes at all is Unknown.
    pub fn dominant(&self) -> AgeBucket {
        let mut best = AgeBucket::Within7Days;
        let mut best_bytes = self.get(best);
        for b in AgeBucket::KNOWN.into_iter().skip(1) {
            if self.get(b) > best_bytes {
                best = b;
                best_bytes = self.get(b);
            }
        }
        if self.get(AgeBucket::Unknown) >= best_bytes {
            AgeBucket::Unknown
        } else {
            best
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference() -> Ticks {
        Ticks::from_filetime(crate::time::UNIX_EPOCH_TICKS + 2_000 * TICKS_PER_DAY).unwrap()
    }

    fn days_ago(d: i64, extra_ticks: i64) -> Option<Ticks> {
        reference().checked_add_ticks(-(d * TICKS_PER_DAY) - extra_ticks)
    }

    #[test]
    fn exact_boundaries_are_inclusive() {
        let r = reference();
        assert_eq!(bucket_for(r, days_ago(0, 0)), AgeBucket::Within7Days);
        assert_eq!(bucket_for(r, days_ago(7, 0)), AgeBucket::Within7Days);
        assert_eq!(bucket_for(r, days_ago(7, 1)), AgeBucket::Days8To30);
        assert_eq!(bucket_for(r, days_ago(30, 0)), AgeBucket::Days8To30);
        assert_eq!(bucket_for(r, days_ago(30, 1)), AgeBucket::Days31To90);
        assert_eq!(bucket_for(r, days_ago(90, 0)), AgeBucket::Days31To90);
        assert_eq!(bucket_for(r, days_ago(90, 1)), AgeBucket::Days91To365);
        assert_eq!(bucket_for(r, days_ago(365, 0)), AgeBucket::Days91To365);
        assert_eq!(bucket_for(r, days_ago(365, 1)), AgeBucket::OverOneYear);
    }

    #[test]
    fn future_missing_and_invalid_are_unknown() {
        let r = reference();
        assert_eq!(bucket_for(r, r.checked_add_ticks(1)), AgeBucket::Unknown);
        assert_eq!(bucket_for(r, None), AgeBucket::Unknown);
        assert_eq!(bucket_for(r, Ticks::from_filetime(0)), AgeBucket::Unknown);
    }

    #[test]
    fn unverified_access_dates_render_unknown_unless_opted_in() {
        let r = reference();
        let ts = days_ago(3, 0);
        let default = AccessPolicy::default();
        assert_eq!(default.bucket(r, ts), AgeBucket::Unknown);
        let opted = AccessPolicy {
            reliability: AccessReliability::Unverified,
            include_unverified: true,
        };
        assert_eq!(opted.bucket(r, ts), AgeBucket::Within7Days);
        let unavailable = AccessPolicy {
            reliability: AccessReliability::Unavailable,
            include_unverified: true,
        };
        assert_eq!(unavailable.bucket(r, ts), AgeBucket::Unknown);
        let known = AccessPolicy {
            reliability: AccessReliability::Known,
            include_unverified: false,
        };
        assert_eq!(known.bucket(r, ts), AgeBucket::Within7Days);
    }

    fn hist(values: [u128; 6]) -> AgeHistogram {
        AgeHistogram(values)
    }

    #[test]
    fn dominant_picks_largest_known() {
        assert_eq!(hist([1, 5, 2, 0, 0, 0]).dominant(), AgeBucket::Days8To30);
        assert_eq!(hist([0, 0, 0, 0, 9, 3]).dominant(), AgeBucket::OverOneYear);
    }

    #[test]
    fn dominant_ties_between_known_go_to_most_recent() {
        assert_eq!(hist([0, 4, 0, 4, 0, 0]).dominant(), AgeBucket::Days8To30);
        assert_eq!(hist([7, 7, 7, 7, 7, 0]).dominant(), AgeBucket::Within7Days);
    }

    #[test]
    fn unknown_wins_when_strictly_largest_or_tied() {
        assert_eq!(hist([1, 2, 3, 4, 5, 6]).dominant(), AgeBucket::Unknown);
        assert_eq!(hist([0, 0, 5, 0, 0, 5]).dominant(), AgeBucket::Unknown);
        assert_eq!(hist([0, 0, 6, 0, 0, 5]).dominant(), AgeBucket::Days31To90);
    }

    #[test]
    fn empty_block_is_unknown() {
        assert_eq!(AgeHistogram::default().dominant(), AgeBucket::Unknown);
    }
}
