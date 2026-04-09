use crate::amount::{Amt, SignedAmt};

#[derive(Clone, Debug)]
pub struct Modifier {
    pub id: String,
    pub label: String,
    pub base_add: SignedAmt,
    pub multiplier: SignedAmt,
    pub add: SignedAmt,
}

#[derive(Clone, Debug, Default)]
pub struct ModifiedValue {
    base: Amt,
    modifiers: Vec<Modifier>,
}

impl ModifiedValue {
    pub fn new(base: Amt) -> Self {
        Self {
            base,
            modifiers: Vec::new(),
        }
    }

    pub fn set_base(&mut self, base: Amt) {
        self.base = base;
    }

    pub fn base(&self) -> Amt {
        self.base
    }

    /// Add a modifier. If a modifier with the same `id` already exists, replace it.
    pub fn push_modifier(&mut self, modifier: Modifier) {
        if let Some(existing) = self.modifiers.iter_mut().find(|m| m.id == modifier.id) {
            *existing = modifier;
        } else {
            self.modifiers.push(modifier);
        }
    }

    /// Remove a modifier by id, returning it if found.
    pub fn pop_modifier(&mut self, id: &str) -> Option<Modifier> {
        if let Some(pos) = self.modifiers.iter().position(|m| m.id == id) {
            Some(self.modifiers.remove(pos))
        } else {
            None
        }
    }

    pub fn has_modifier(&self, id: &str) -> bool {
        self.modifiers.iter().any(|m| m.id == id)
    }

    pub fn modifiers(&self) -> &[Modifier] {
        &self.modifiers
    }

    /// `base + Σ base_add`, clamped to 0
    pub fn effective_base(&self) -> Amt {
        let mut sum = self.base.raw() as i64;
        for m in &self.modifiers {
            sum += m.base_add.raw();
        }
        if sum < 0 {
            Amt::ZERO
        } else {
            Amt(sum as u64)
        }
    }

    /// `1.000 + Σ multiplier` (as SignedAmt for display; clamped to 0 in final_value)
    pub fn total_multiplier(&self) -> SignedAmt {
        let mut sum = SignedAmt::units(1);
        for m in &self.modifiers {
            sum = sum.add(m.multiplier);
        }
        sum
    }

    /// `Σ add`
    pub fn total_add(&self) -> SignedAmt {
        let mut sum = SignedAmt::ZERO;
        for m in &self.modifiers {
            sum = sum.add(m.add);
        }
        sum
    }

    /// `(base + Σ base_add) * (1.000 + Σ multiplier) + Σ add`, clamped to 0
    pub fn final_value(&self) -> Amt {
        let eb = self.effective_base();
        let tm = self.total_multiplier();
        // Clamp multiplier to 0 — negative multiplier means "reduce to zero"
        let tm_raw = tm.raw().max(0) as i128;
        let product = eb.raw() as i128 * tm_raw / 1000;
        let result = product + self.total_add().raw() as i128;
        if result < 0 {
            Amt::ZERO
        } else {
            Amt(result as u64)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_modifier(id: &str, base_add: SignedAmt, multiplier: SignedAmt, add: SignedAmt) -> Modifier {
        Modifier {
            id: id.to_string(),
            label: id.to_string(),
            base_add,
            multiplier,
            add,
        }
    }

    #[test]
    fn basic_no_modifiers() {
        let mv = ModifiedValue::new(Amt::units(10));
        assert_eq!(mv.base(), Amt::units(10));
        assert_eq!(mv.effective_base(), Amt::units(10));
        assert_eq!(mv.total_multiplier(), SignedAmt::units(1));
        assert_eq!(mv.total_add(), SignedAmt::ZERO);
        assert_eq!(mv.final_value(), Amt::units(10));
    }

    #[test]
    fn base_add_modifiers() {
        let mut mv = ModifiedValue::new(Amt::units(5));
        mv.push_modifier(make_modifier("mine_0", SignedAmt::units(3), SignedAmt::ZERO, SignedAmt::ZERO));
        assert_eq!(mv.effective_base(), Amt::units(8));
        assert_eq!(mv.final_value(), Amt::units(8));
    }

    #[test]
    fn multiplier_modifiers() {
        let mut mv = ModifiedValue::new(Amt::units(10));
        // +15% = 0.150
        mv.push_modifier(make_modifier("tech_mining", SignedAmt::ZERO, SignedAmt::new(0, 150), SignedAmt::ZERO));
        assert_eq!(mv.total_multiplier(), SignedAmt::new(1, 150));
        assert_eq!(mv.final_value(), Amt::new(11, 500));
    }

    #[test]
    fn add_modifiers() {
        let mut mv = ModifiedValue::new(Amt::units(10));
        mv.push_modifier(make_modifier("flat_bonus", SignedAmt::ZERO, SignedAmt::ZERO, SignedAmt::units(2)));
        assert_eq!(mv.total_add(), SignedAmt::units(2));
        assert_eq!(mv.final_value(), Amt::units(12));
    }

    #[test]
    fn combined_modifiers() {
        // base=5, base_add=3, mult=+15%, add=0 → (5+3)*1.15 = 9.2
        let mut mv = ModifiedValue::new(Amt::units(5));
        mv.push_modifier(make_modifier("mine_0", SignedAmt::units(3), SignedAmt::ZERO, SignedAmt::ZERO));
        mv.push_modifier(make_modifier("tech_auto", SignedAmt::ZERO, SignedAmt::new(0, 150), SignedAmt::ZERO));
        assert_eq!(mv.effective_base(), Amt::units(8));
        assert_eq!(mv.total_multiplier(), SignedAmt::new(1, 150));
        assert_eq!(mv.final_value(), Amt::new(9, 200));
    }

    #[test]
    fn push_modifier_replaces_same_id() {
        let mut mv = ModifiedValue::new(Amt::units(5));
        mv.push_modifier(make_modifier("mine_0", SignedAmt::units(3), SignedAmt::ZERO, SignedAmt::ZERO));
        assert_eq!(mv.final_value(), Amt::units(8));
        // Replace with different value
        mv.push_modifier(make_modifier("mine_0", SignedAmt::units(5), SignedAmt::ZERO, SignedAmt::ZERO));
        assert_eq!(mv.modifiers().len(), 1);
        assert_eq!(mv.final_value(), Amt::units(10));
    }

    #[test]
    fn pop_modifier_removes_and_returns() {
        let mut mv = ModifiedValue::new(Amt::units(5));
        mv.push_modifier(make_modifier("mine_0", SignedAmt::units(3), SignedAmt::ZERO, SignedAmt::ZERO));
        assert!(mv.has_modifier("mine_0"));

        let removed = mv.pop_modifier("mine_0");
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().id, "mine_0");
        assert!(!mv.has_modifier("mine_0"));
        assert_eq!(mv.final_value(), Amt::units(5));

        // Removing non-existent returns None
        assert!(mv.pop_modifier("nonexistent").is_none());
    }

    #[test]
    fn multiple_modifiers_summed() {
        let mut mv = ModifiedValue::new(Amt::units(2));
        mv.push_modifier(make_modifier("mine_0", SignedAmt::units(1), SignedAmt::ZERO, SignedAmt::ZERO));
        mv.push_modifier(make_modifier("mine_1", SignedAmt::units(2), SignedAmt::ZERO, SignedAmt::ZERO));
        mv.push_modifier(make_modifier("tech_a", SignedAmt::ZERO, SignedAmt::new(0, 100), SignedAmt::ZERO));
        mv.push_modifier(make_modifier("tech_b", SignedAmt::ZERO, SignedAmt::new(0, 200), SignedAmt::ZERO));
        mv.push_modifier(make_modifier("bonus_a", SignedAmt::ZERO, SignedAmt::ZERO, SignedAmt::units(1)));
        mv.push_modifier(make_modifier("bonus_b", SignedAmt::ZERO, SignedAmt::ZERO, SignedAmt::new(0, 500)));

        // effective_base = 2 + 1 + 2 = 5
        assert_eq!(mv.effective_base(), Amt::units(5));
        // total_multiplier = 1.000 + 0.100 + 0.200 = 1.300
        assert_eq!(mv.total_multiplier(), SignedAmt::new(1, 300));
        // total_add = 1.000 + 0.500 = 1.500
        assert_eq!(mv.total_add(), SignedAmt::new(1, 500));
        // final = 5 * 1.3 + 1.5 = 6.5 + 1.5 = 8.0
        assert_eq!(mv.final_value(), Amt::units(8));
    }

    #[test]
    fn set_base_updates() {
        let mut mv = ModifiedValue::new(Amt::units(5));
        assert_eq!(mv.base(), Amt::units(5));
        mv.set_base(Amt::units(10));
        assert_eq!(mv.base(), Amt::units(10));
        assert_eq!(mv.final_value(), Amt::units(10));
    }

    #[test]
    fn default_is_zero() {
        let mv = ModifiedValue::default();
        assert_eq!(mv.base(), Amt::ZERO);
        assert_eq!(mv.final_value(), Amt::ZERO);
    }

    // --- Negative modifier tests ---

    #[test]
    fn test_negative_multiplier() {
        // base=10, multiplier=-0.200 → total_mult=0.8 → final = 10 * 0.8 = 8
        let mut mv = ModifiedValue::new(Amt::units(10));
        mv.push_modifier(make_modifier("debuff", SignedAmt::ZERO, SignedAmt::new(0, -200), SignedAmt::ZERO));
        assert_eq!(mv.total_multiplier(), SignedAmt::new(0, 800));
        assert_eq!(mv.final_value(), Amt::units(8));
    }

    #[test]
    fn test_negative_base_add() {
        // base=5, base_add=-3 → effective_base=2, final=2
        let mut mv = ModifiedValue::new(Amt::units(5));
        mv.push_modifier(make_modifier("penalty", SignedAmt::units(-3), SignedAmt::ZERO, SignedAmt::ZERO));
        assert_eq!(mv.effective_base(), Amt::units(2));
        assert_eq!(mv.final_value(), Amt::units(2));
    }

    #[test]
    fn test_negative_add() {
        // base=10, add=-3 → final=7
        let mut mv = ModifiedValue::new(Amt::units(10));
        mv.push_modifier(make_modifier("tax", SignedAmt::ZERO, SignedAmt::ZERO, SignedAmt::units(-3)));
        assert_eq!(mv.final_value(), Amt::units(7));
    }

    #[test]
    fn test_clamp_to_zero() {
        // base=5, multiplier=-2.0 → total_mult = 1 + (-2) = -1 → clamped to 0 → final=0
        let mut mv = ModifiedValue::new(Amt::units(5));
        mv.push_modifier(make_modifier("destroy", SignedAmt::ZERO, SignedAmt::units(-2), SignedAmt::ZERO));
        assert_eq!(mv.final_value(), Amt::ZERO);
    }

    #[test]
    fn test_clamp_base_to_zero() {
        // base=3, base_add=-10 → effective_base clamped to 0 → final=0
        let mut mv = ModifiedValue::new(Amt::units(3));
        mv.push_modifier(make_modifier("drain", SignedAmt::units(-10), SignedAmt::ZERO, SignedAmt::ZERO));
        assert_eq!(mv.effective_base(), Amt::ZERO);
        assert_eq!(mv.final_value(), Amt::ZERO);
    }

    #[test]
    fn test_clamp_add_to_zero() {
        // base=5, add=-10 → result clamped to 0
        let mut mv = ModifiedValue::new(Amt::units(5));
        mv.push_modifier(make_modifier("tax", SignedAmt::ZERO, SignedAmt::ZERO, SignedAmt::units(-10)));
        assert_eq!(mv.final_value(), Amt::ZERO);
    }

    #[test]
    fn test_mixed_positive_negative() {
        // base=10, mod1 mult=+0.5, mod2 mult=-0.3 → total_mult=1.0+0.5-0.3=1.2 → final=12
        let mut mv = ModifiedValue::new(Amt::units(10));
        mv.push_modifier(make_modifier("buff", SignedAmt::ZERO, SignedAmt::new(0, 500), SignedAmt::ZERO));
        mv.push_modifier(make_modifier("nerf", SignedAmt::ZERO, SignedAmt::new(0, -300), SignedAmt::ZERO));
        assert_eq!(mv.total_multiplier(), SignedAmt::new(1, 200));
        assert_eq!(mv.final_value(), Amt::units(12));
    }
}
