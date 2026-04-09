use crate::amount::Amt;

#[derive(Clone, Debug)]
pub struct Modifier {
    pub id: String,
    pub label: String,
    pub base_add: Amt,
    pub multiplier: Amt,
    pub add: Amt,
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

    /// `base + Σ base_add`
    pub fn effective_base(&self) -> Amt {
        let mut sum = self.base;
        for m in &self.modifiers {
            sum = sum.add(m.base_add);
        }
        sum
    }

    /// `1.000 + Σ multiplier`
    pub fn total_multiplier(&self) -> Amt {
        let mut sum = Amt::units(1);
        for m in &self.modifiers {
            sum = sum.add(m.multiplier);
        }
        sum
    }

    /// `Σ add`
    pub fn total_add(&self) -> Amt {
        let mut sum = Amt::ZERO;
        for m in &self.modifiers {
            sum = sum.add(m.add);
        }
        sum
    }

    /// `(base + Σ base_add) * (1.000 + Σ multiplier) + Σ add`
    pub fn final_value(&self) -> Amt {
        self.effective_base()
            .mul_amt(self.total_multiplier())
            .add(self.total_add())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_modifier(id: &str, base_add: Amt, multiplier: Amt, add: Amt) -> Modifier {
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
        assert_eq!(mv.total_multiplier(), Amt::units(1));
        assert_eq!(mv.total_add(), Amt::ZERO);
        assert_eq!(mv.final_value(), Amt::units(10));
    }

    #[test]
    fn base_add_modifiers() {
        let mut mv = ModifiedValue::new(Amt::units(5));
        mv.push_modifier(make_modifier("mine_0", Amt::units(3), Amt::ZERO, Amt::ZERO));
        assert_eq!(mv.effective_base(), Amt::units(8));
        assert_eq!(mv.final_value(), Amt::units(8));
    }

    #[test]
    fn multiplier_modifiers() {
        let mut mv = ModifiedValue::new(Amt::units(10));
        // +15% = 0.150
        mv.push_modifier(make_modifier("tech_mining", Amt::ZERO, Amt::new(0, 150), Amt::ZERO));
        assert_eq!(mv.total_multiplier(), Amt::new(1, 150));
        assert_eq!(mv.final_value(), Amt::new(11, 500));
    }

    #[test]
    fn add_modifiers() {
        let mut mv = ModifiedValue::new(Amt::units(10));
        mv.push_modifier(make_modifier("flat_bonus", Amt::ZERO, Amt::ZERO, Amt::units(2)));
        assert_eq!(mv.total_add(), Amt::units(2));
        assert_eq!(mv.final_value(), Amt::units(12));
    }

    #[test]
    fn combined_modifiers() {
        // base=5, base_add=3, mult=+15%, add=0 → (5+3)*1.15 = 9.2
        let mut mv = ModifiedValue::new(Amt::units(5));
        mv.push_modifier(make_modifier("mine_0", Amt::units(3), Amt::ZERO, Amt::ZERO));
        mv.push_modifier(make_modifier("tech_auto", Amt::ZERO, Amt::new(0, 150), Amt::ZERO));
        assert_eq!(mv.effective_base(), Amt::units(8));
        assert_eq!(mv.total_multiplier(), Amt::new(1, 150));
        assert_eq!(mv.final_value(), Amt::new(9, 200));
    }

    #[test]
    fn push_modifier_replaces_same_id() {
        let mut mv = ModifiedValue::new(Amt::units(5));
        mv.push_modifier(make_modifier("mine_0", Amt::units(3), Amt::ZERO, Amt::ZERO));
        assert_eq!(mv.final_value(), Amt::units(8));
        // Replace with different value
        mv.push_modifier(make_modifier("mine_0", Amt::units(5), Amt::ZERO, Amt::ZERO));
        assert_eq!(mv.modifiers().len(), 1);
        assert_eq!(mv.final_value(), Amt::units(10));
    }

    #[test]
    fn pop_modifier_removes_and_returns() {
        let mut mv = ModifiedValue::new(Amt::units(5));
        mv.push_modifier(make_modifier("mine_0", Amt::units(3), Amt::ZERO, Amt::ZERO));
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
        mv.push_modifier(make_modifier("mine_0", Amt::units(1), Amt::ZERO, Amt::ZERO));
        mv.push_modifier(make_modifier("mine_1", Amt::units(2), Amt::ZERO, Amt::ZERO));
        mv.push_modifier(make_modifier("tech_a", Amt::ZERO, Amt::new(0, 100), Amt::ZERO));
        mv.push_modifier(make_modifier("tech_b", Amt::ZERO, Amt::new(0, 200), Amt::ZERO));
        mv.push_modifier(make_modifier("bonus_a", Amt::ZERO, Amt::ZERO, Amt::units(1)));
        mv.push_modifier(make_modifier("bonus_b", Amt::ZERO, Amt::ZERO, Amt::new(0, 500)));

        // effective_base = 2 + 1 + 2 = 5
        assert_eq!(mv.effective_base(), Amt::units(5));
        // total_multiplier = 1.000 + 0.100 + 0.200 = 1.300
        assert_eq!(mv.total_multiplier(), Amt::new(1, 300));
        // total_add = 1.000 + 0.500 = 1.500
        assert_eq!(mv.total_add(), Amt::new(1, 500));
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
}
