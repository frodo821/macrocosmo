/// Fixed-point resource amount. 1 displayed unit = 1000 internal units.
/// All arithmetic is pure integer — no floating point.
///
/// Display: `Amt(5500)` → "5.5", `Amt(5000)` → "5", `Amt(123)` → "0.123"

const SCALE: u64 = 1000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Amt(pub u64);

impl Amt {
    pub const ZERO: Amt = Amt(0);

    /// Create from whole units: `Amt::units(5)` = 5.000
    pub const fn units(n: u64) -> Self {
        Amt(n * SCALE)
    }

    /// Create from milli-units directly: `Amt::milli(5500)` = 5.500
    pub const fn milli(n: u64) -> Self {
        Amt(n)
    }

    /// Create from whole + fractional: `Amt::new(5, 500)` = 5.500
    pub const fn new(whole: u64, millis: u64) -> Self {
        Amt(whole * SCALE + millis)
    }

    /// Convert from f64 (for bridging with population and other f64 values).
    /// Negative values clamp to 0.
    pub fn from_f64(v: f64) -> Self {
        if v <= 0.0 {
            Amt::ZERO
        } else {
            Amt((v * SCALE as f64) as u64)
        }
    }

    /// Convert to f64 (for bridging with population calculations).
    pub fn to_f64(self) -> f64 {
        self.0 as f64 / SCALE as f64
    }

    /// Whole part: `Amt(5500).whole()` = 5
    pub const fn whole(self) -> u64 {
        self.0 / SCALE
    }

    /// Fractional milli part: `Amt(5500).frac()` = 500
    pub const fn frac(self) -> u64 {
        self.0 % SCALE
    }

    /// Raw internal value.
    pub const fn raw(self) -> u64 {
        self.0
    }

    /// Saturating addition.
    pub const fn add(self, rhs: Amt) -> Amt {
        Amt(self.0.saturating_add(rhs.0))
    }

    /// Saturating subtraction (floors at 0).
    pub const fn sub(self, rhs: Amt) -> Amt {
        Amt(self.0.saturating_sub(rhs.0))
    }

    /// Multiply by an integer scalar.
    pub const fn mul_u64(self, n: u64) -> Amt {
        Amt(self.0.saturating_mul(n))
    }

    /// Fixed-point multiply: (a * b) / SCALE.
    /// Useful for rate × time, or amount × ratio.
    pub const fn mul_amt(self, rhs: Amt) -> Amt {
        // Use u128 to avoid overflow on intermediate product
        let product = self.0 as u128 * rhs.0 as u128;
        let result = product / SCALE as u128;
        // Saturate to u64::MAX
        if result > u64::MAX as u128 {
            Amt(u64::MAX)
        } else {
            Amt(result as u64)
        }
    }

    /// Fixed-point divide: (a * SCALE) / b.
    /// Returns Amt::ZERO if rhs is zero.
    pub const fn div_amt(self, rhs: Amt) -> Amt {
        if rhs.0 == 0 {
            return Amt::ZERO;
        }
        let numer = self.0 as u128 * SCALE as u128;
        let result = numer / rhs.0 as u128;
        if result > u64::MAX as u128 {
            Amt(u64::MAX)
        } else {
            Amt(result as u64)
        }
    }

    /// Clamp to a maximum value.
    pub const fn min(self, other: Amt) -> Amt {
        if self.0 < other.0 { self } else { other }
    }

    /// Take the larger value.
    pub const fn max(self, other: Amt) -> Amt {
        if self.0 > other.0 { self } else { other }
    }

    /// Format for display. "5" for whole, "5.5" for non-zero fraction, "0.123" for sub-unit.
    pub fn display(self) -> String {
        let w = self.whole();
        let f = self.frac();
        if f == 0 {
            format!("{}", w)
        } else if f % 100 == 0 {
            format!("{}.{}", w, f / 100)
        } else if f % 10 == 0 {
            format!("{}.{:02}", w, f / 10)
        } else {
            format!("{}.{:03}", w, f)
        }
    }

    /// Format with sign for net income display. "+5.5" or "-3.2"
    pub fn display_signed(self, positive: bool) -> String {
        if positive {
            format!("+{}", self.display())
        } else {
            format!("-{}", self.display())
        }
    }
}

impl std::fmt::Display for Amt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructors() {
        assert_eq!(Amt::units(5), Amt(5000));
        assert_eq!(Amt::new(5, 500), Amt(5500));
        assert_eq!(Amt::milli(123), Amt(123));
        assert_eq!(Amt::ZERO, Amt(0));
    }

    #[test]
    fn whole_and_frac() {
        assert_eq!(Amt(5500).whole(), 5);
        assert_eq!(Amt(5500).frac(), 500);
        assert_eq!(Amt(123).whole(), 0);
        assert_eq!(Amt(123).frac(), 123);
    }

    #[test]
    fn arithmetic() {
        assert_eq!(Amt::units(3).add(Amt::units(2)), Amt::units(5));
        assert_eq!(Amt::units(3).sub(Amt::units(5)), Amt::ZERO); // saturates
        assert_eq!(Amt::units(3).mul_u64(4), Amt::units(12));
    }

    #[test]
    fn fixed_point_mul() {
        // 5.0 * 3.0 = 15.0
        assert_eq!(Amt::units(5).mul_amt(Amt::units(3)), Amt::units(15));
        // 2.5 * 4.0 = 10.0
        assert_eq!(Amt::new(2, 500).mul_amt(Amt::units(4)), Amt::units(10));
        // 1.5 * 1.5 = 2.25
        assert_eq!(Amt::new(1, 500).mul_amt(Amt::new(1, 500)), Amt::new(2, 250));
    }

    #[test]
    fn fixed_point_div() {
        // 10.0 / 2.0 = 5.0
        assert_eq!(Amt::units(10).div_amt(Amt::units(2)), Amt::units(5));
        // 1.0 / 3.0 = 0.333
        assert_eq!(Amt::units(1).div_amt(Amt::units(3)), Amt(333));
        // div by zero → ZERO
        assert_eq!(Amt::units(1).div_amt(Amt::ZERO), Amt::ZERO);
    }

    #[test]
    fn display_formatting() {
        assert_eq!(Amt::units(5).display(), "5");
        assert_eq!(Amt::new(5, 500).display(), "5.5");
        assert_eq!(Amt::new(5, 250).display(), "5.25");
        assert_eq!(Amt(123).display(), "0.123");
        assert_eq!(Amt::ZERO.display(), "0");
    }

    #[test]
    fn display_signed_formatting() {
        assert_eq!(Amt::new(3, 200).display_signed(true), "+3.2");
        assert_eq!(Amt::new(3, 200).display_signed(false), "-3.2");
    }

    #[test]
    fn min_max() {
        assert_eq!(Amt::units(3).min(Amt::units(5)), Amt::units(3));
        assert_eq!(Amt::units(3).max(Amt::units(5)), Amt::units(5));
    }

    #[test]
    fn saturation() {
        assert_eq!(Amt(u64::MAX).add(Amt::units(1)), Amt(u64::MAX));
        assert_eq!(Amt(u64::MAX).mul_u64(2), Amt(u64::MAX));
    }
}
