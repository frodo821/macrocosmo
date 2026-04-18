//! Nash equilibrium solver — **Phase 2 stub**.
//!
//! The real 2-player zero-sum solver (#190) will live here. For now we provide
//! the type shapes so downstream code can compile and test against the
//! interface, returning zero-payoff placeholders.

use serde::{Deserialize, Serialize};

/// A payoff pair: `row` is the row player's utility, `col` is the column
/// player's utility. In a zero-sum game `col = -row`.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct NashPayoff {
    pub row: f64,
    pub col: f64,
}

/// Solve a 2x2 2-player zero-sum game.
///
/// **TODO Phase 3**: implement a minimax / LP-based solver. For now this
/// returns `NashPayoff { row: 0.0, col: 0.0 }` regardless of input.
#[allow(unused_variables)]
pub fn solve_2p_zero_sum(payoff_matrix: &[[f64; 2]; 2]) -> NashPayoff {
    NashPayoff { row: 0.0, col: 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_returns_zero() {
        let m = [[1.0, -1.0], [-1.0, 1.0]];
        assert_eq!(solve_2p_zero_sum(&m), NashPayoff::default());
    }

    #[test]
    fn payoff_is_default_zero() {
        let p = NashPayoff::default();
        assert_eq!(p.row, 0.0);
        assert_eq!(p.col, 0.0);
    }
}
