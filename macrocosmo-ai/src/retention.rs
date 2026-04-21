//! Retention policies for time-series topic history.
//!
//! Named variants match the four canonical windows from #192 `DelT` design.
//! `Custom(Tick)` allows arbitrary overrides without forcing a named bucket.

use serde::{Deserialize, Serialize};

use crate::time::Tick;

/// Retention window for a time-series topic. Old samples are evicted when
/// they fall outside this window relative to the newest emitted sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Retention {
    /// 30 hexadies (~short-term situational awareness).
    Short,
    /// 120 hexadies.
    Medium,
    /// 500 hexadies.
    Long,
    /// 1200 hexadies (maximum DelT window).
    VeryLong,
    /// Arbitrary retention in hexadies.
    Custom(Tick),
}

impl Retention {
    /// Length of the retention window in ticks (hexadies).
    pub fn as_ticks(self) -> Tick {
        match self {
            Retention::Short => 30,
            Retention::Medium => 120,
            Retention::Long => 500,
            Retention::VeryLong => 1200,
            Retention::Custom(t) => t,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_retentions_map_to_expected_ticks() {
        assert_eq!(Retention::Short.as_ticks(), 30);
        assert_eq!(Retention::Medium.as_ticks(), 120);
        assert_eq!(Retention::Long.as_ticks(), 500);
        assert_eq!(Retention::VeryLong.as_ticks(), 1200);
        assert_eq!(Retention::Custom(77).as_ticks(), 77);
    }
}
