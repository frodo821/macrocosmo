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

// ---------------------------------------------------------------------------
// SignedAmt — signed fixed-point for modifier deltas
// ---------------------------------------------------------------------------

const SIGNED_SCALE: i64 = 1000;

/// Signed fixed-point amount. 1 displayed unit = 1000 internal units.
/// Used for modifier values that can be negative (e.g., -20% = SignedAmt(-200)).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SignedAmt(pub i64);

impl SignedAmt {
    pub const ZERO: SignedAmt = SignedAmt(0);

    /// Create from whole units: `SignedAmt::units(-3)` = -3.000
    pub const fn units(n: i64) -> Self {
        SignedAmt(n * SIGNED_SCALE)
    }

    /// Create from whole + fractional millis: `SignedAmt::new(0, -200)` = -0.200
    pub const fn new(whole: i64, millis: i64) -> Self {
        SignedAmt(whole * SIGNED_SCALE + millis)
    }

    /// Create from milli-units directly: `SignedAmt::milli(-500)` = -0.500
    pub const fn milli(n: i64) -> Self {
        SignedAmt(n)
    }

    /// Convert from an unsigned `Amt` (always non-negative).
    pub const fn from_amt(a: Amt) -> Self {
        SignedAmt(a.0 as i64)
    }

    /// Raw internal value.
    pub const fn raw(self) -> i64 {
        self.0
    }

    /// Signed addition.
    pub const fn add(self, rhs: SignedAmt) -> SignedAmt {
        SignedAmt(self.0 + rhs.0)
    }

    /// Format for display with explicit sign. "+5.5", "-0.2", "0"
    pub fn display(self) -> String {
        if self.0 == 0 {
            return "0".to_string();
        }
        let sign = if self.0 < 0 { "-" } else { "+" };
        let abs = self.0.unsigned_abs();
        let w = abs / SIGNED_SCALE as u64;
        let f = abs % SIGNED_SCALE as u64;
        if f == 0 {
            format!("{}{}", sign, w)
        } else if f % 100 == 0 {
            format!("{}{}.{}", sign, w, f / 100)
        } else if f % 10 == 0 {
            format!("{}{}.{:02}", sign, w, f / 10)
        } else {
            format!("{}{}.{:03}", sign, w, f)
        }
    }

    /// Format as percentage. "+15%", "-20%"
    pub fn display_as_percent(self) -> String {
        if self.0 == 0 {
            return "0%".to_string();
        }
        let sign = if self.0 < 0 { "-" } else { "+" };
        let abs = self.0.unsigned_abs();
        // 150 millis = 15.0%, 200 millis = 20.0%
        let whole_pct = abs / 10;
        let frac_pct = abs % 10;
        if frac_pct == 0 {
            format!("{}{}%", sign, whole_pct)
        } else {
            format!("{}{}.{}%", sign, whole_pct, frac_pct)
        }
    }
}

impl std::fmt::Display for SignedAmt {
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

    // --- SignedAmt tests ---

    #[test]
    fn signed_constructors() {
        assert_eq!(SignedAmt::units(5), SignedAmt(5000));
        assert_eq!(SignedAmt::units(-3), SignedAmt(-3000));
        assert_eq!(SignedAmt::new(0, -200), SignedAmt(-200));
        assert_eq!(SignedAmt::new(1, 500), SignedAmt(1500));
        assert_eq!(SignedAmt::milli(-123), SignedAmt(-123));
        assert_eq!(SignedAmt::ZERO, SignedAmt(0));
    }

    #[test]
    fn signed_from_amt() {
        assert_eq!(SignedAmt::from_amt(Amt::units(5)), SignedAmt(5000));
        assert_eq!(SignedAmt::from_amt(Amt::ZERO), SignedAmt::ZERO);
    }

    #[test]
    fn signed_add() {
        assert_eq!(SignedAmt::units(3).add(SignedAmt::units(-5)), SignedAmt::units(-2));
        assert_eq!(SignedAmt::units(-1).add(SignedAmt::units(1)), SignedAmt::ZERO);
    }

    #[test]
    fn signed_display() {
        assert_eq!(SignedAmt::units(5).display(), "+5");
        assert_eq!(SignedAmt::new(5, 500).display(), "+5.5");
        assert_eq!(SignedAmt::new(0, -200).display(), "-0.2");
        assert_eq!(SignedAmt::ZERO.display(), "0");
        assert_eq!(SignedAmt::units(-3).display(), "-3");
    }

    #[test]
    fn signed_display_as_percent() {
        assert_eq!(SignedAmt::new(0, 150).display_as_percent(), "+15%");
        assert_eq!(SignedAmt::new(0, -200).display_as_percent(), "-20%");
        assert_eq!(SignedAmt::ZERO.display_as_percent(), "0%");
    }
}
