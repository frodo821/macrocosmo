use crate::amount::{Amt, SignedAmt};

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "reflect", derive(bevy_reflect::Reflect))]
pub struct Modifier {
    pub id: String,
    pub label: String,
    pub base_add: SignedAmt,
    pub multiplier: SignedAmt,
    pub add: SignedAmt,
    /// None = permanent, Some(t) = expires when clock.elapsed >= t.
    pub expires_at: Option<i64>,
    /// Optional event id to fire when this modifier expires.
    pub on_expire_event: Option<String>,
}

impl Modifier {
    /// Returns remaining hexadies until expiration, or None if permanent.
    pub fn remaining_duration(&self, current_time: i64) -> Option<i64> {
        self.expires_at.map(|t| (t - current_time).max(0))
    }
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "reflect", derive(bevy_reflect::Reflect))]
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

    /// Push a modifier that expires after `duration` hexadies from `now`.
    pub fn push_modifier_timed(&mut self, mut modifier: Modifier, now: i64, duration: i64) {
        modifier.expires_at = Some(now + duration);
        self.push_modifier(modifier);
    }

    /// Remove all expired modifiers, returning them.
    pub fn drain_expired(&mut self, current_time: i64) -> Vec<Modifier> {
        let mut expired = Vec::new();
        let mut i = 0;
        while i < self.modifiers.len() {
            if let Some(t) = self.modifiers[i].expires_at
                && t <= current_time
            {
                expired.push(self.modifiers.remove(i));
                continue;
            }
            i += 1;
        }
        expired
    }

    /// Remove all modifiers whose expires_at <= current_time. Returns count removed.
    pub fn cleanup_expired(&mut self, current_time: i64) -> usize {
        self.drain_expired(current_time).len()
    }

    /// `base + sum(base_add)`, clamped to 0.
    pub fn effective_base(&self) -> Amt {
        let mut sum = self.base.raw() as i64;
        for m in &self.modifiers {
            sum += m.base_add.raw();
        }
        if sum < 0 { Amt::ZERO } else { Amt(sum as u64) }
    }

    /// `1.000 + sum(multiplier)`.
    pub fn total_multiplier(&self) -> SignedAmt {
        let mut sum = SignedAmt::units(1);
        for m in &self.modifiers {
            sum = sum.add(m.multiplier);
        }
        sum
    }

    /// `sum(add)`.
    pub fn total_add(&self) -> SignedAmt {
        let mut sum = SignedAmt::ZERO;
        for m in &self.modifiers {
            sum = sum.add(m.add);
        }
        sum
    }

    /// `(base + sum(base_add)) * (1.000 + sum(multiplier)) + sum(add)`, clamped to 0.
    pub fn final_value(&self) -> Amt {
        let eb = self.effective_base();
        let tm = self.total_multiplier();
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

    fn modifier(id: &str, base_add: SignedAmt, multiplier: SignedAmt, add: SignedAmt) -> Modifier {
        Modifier {
            id: id.to_string(),
            label: id.to_string(),
            base_add,
            multiplier,
            add,
            expires_at: None,
            on_expire_event: None,
        }
    }

    #[test]
    fn modified_value_applies_base_multiplier_and_add() {
        let mut value = ModifiedValue::new(Amt::units(10));
        value.push_modifier(modifier(
            "a",
            SignedAmt::units(2),
            SignedAmt::new(0, 500),
            SignedAmt::units(1),
        ));

        assert_eq!(value.effective_base(), Amt::units(12));
        assert_eq!(value.total_multiplier(), SignedAmt::new(1, 500));
        assert_eq!(value.total_add(), SignedAmt::units(1));
        assert_eq!(value.final_value(), Amt::units(19));
    }

    #[test]
    fn basic_no_modifiers() {
        let value = ModifiedValue::new(Amt::units(10));
        assert_eq!(value.base(), Amt::units(10));
        assert_eq!(value.effective_base(), Amt::units(10));
        assert_eq!(value.total_multiplier(), SignedAmt::units(1));
        assert_eq!(value.total_add(), SignedAmt::ZERO);
        assert_eq!(value.final_value(), Amt::units(10));
    }

    #[test]
    fn channel_specific_modifiers_apply_independently() {
        let mut base_add = ModifiedValue::new(Amt::units(5));
        base_add.push_modifier(modifier(
            "base",
            SignedAmt::units(3),
            SignedAmt::ZERO,
            SignedAmt::ZERO,
        ));
        assert_eq!(base_add.effective_base(), Amt::units(8));
        assert_eq!(base_add.final_value(), Amt::units(8));

        let mut multiplier = ModifiedValue::new(Amt::units(10));
        multiplier.push_modifier(modifier(
            "mult",
            SignedAmt::ZERO,
            SignedAmt::new(0, 150),
            SignedAmt::ZERO,
        ));
        assert_eq!(multiplier.total_multiplier(), SignedAmt::new(1, 150));
        assert_eq!(multiplier.final_value(), Amt::new(11, 500));

        let mut add = ModifiedValue::new(Amt::units(10));
        add.push_modifier(modifier(
            "add",
            SignedAmt::ZERO,
            SignedAmt::ZERO,
            SignedAmt::units(2),
        ));
        assert_eq!(add.total_add(), SignedAmt::units(2));
        assert_eq!(add.final_value(), Amt::units(12));
    }

    #[test]
    fn combined_modifiers_are_summed_by_channel() {
        let mut value = ModifiedValue::new(Amt::units(2));
        value.push_modifier(modifier(
            "base_a",
            SignedAmt::units(1),
            SignedAmt::ZERO,
            SignedAmt::ZERO,
        ));
        value.push_modifier(modifier(
            "base_b",
            SignedAmt::units(2),
            SignedAmt::ZERO,
            SignedAmt::ZERO,
        ));
        value.push_modifier(modifier(
            "mult_a",
            SignedAmt::ZERO,
            SignedAmt::new(0, 100),
            SignedAmt::ZERO,
        ));
        value.push_modifier(modifier(
            "mult_b",
            SignedAmt::ZERO,
            SignedAmt::new(0, 200),
            SignedAmt::ZERO,
        ));
        value.push_modifier(modifier(
            "add_a",
            SignedAmt::ZERO,
            SignedAmt::ZERO,
            SignedAmt::units(1),
        ));
        value.push_modifier(modifier(
            "add_b",
            SignedAmt::ZERO,
            SignedAmt::ZERO,
            SignedAmt::new(0, 500),
        ));

        assert_eq!(value.effective_base(), Amt::units(5));
        assert_eq!(value.total_multiplier(), SignedAmt::new(1, 300));
        assert_eq!(value.total_add(), SignedAmt::new(1, 500));
        assert_eq!(value.final_value(), Amt::units(8));
    }

    #[test]
    fn push_replaces_same_id_and_pop_returns_removed_modifier() {
        let mut value = ModifiedValue::new(Amt::units(5));
        value.push_modifier(modifier(
            "same",
            SignedAmt::units(3),
            SignedAmt::ZERO,
            SignedAmt::ZERO,
        ));
        assert_eq!(value.final_value(), Amt::units(8));

        value.push_modifier(modifier(
            "same",
            SignedAmt::units(5),
            SignedAmt::ZERO,
            SignedAmt::ZERO,
        ));
        assert_eq!(value.modifiers().len(), 1);
        assert_eq!(value.final_value(), Amt::units(10));

        let removed = value.pop_modifier("same").expect("removed");
        assert_eq!(removed.id, "same");
        assert_eq!(value.final_value(), Amt::units(5));
        assert_eq!(value.pop_modifier("missing"), None);
    }

    #[test]
    fn set_base_and_default_behaviour() {
        let mut value = ModifiedValue::new(Amt::units(5));
        value.set_base(Amt::units(10));
        assert_eq!(value.base(), Amt::units(10));
        assert_eq!(value.final_value(), Amt::units(10));

        let default = ModifiedValue::default();
        assert_eq!(default.base(), Amt::ZERO);
        assert_eq!(default.final_value(), Amt::ZERO);
    }

    #[test]
    fn negative_modifiers_and_clamps() {
        let mut multiplier = ModifiedValue::new(Amt::units(10));
        multiplier.push_modifier(modifier(
            "debuff",
            SignedAmt::ZERO,
            SignedAmt::new(0, -200),
            SignedAmt::ZERO,
        ));
        assert_eq!(multiplier.total_multiplier(), SignedAmt::new(0, 800));
        assert_eq!(multiplier.final_value(), Amt::units(8));

        let mut base = ModifiedValue::new(Amt::units(5));
        base.push_modifier(modifier(
            "penalty",
            SignedAmt::units(-3),
            SignedAmt::ZERO,
            SignedAmt::ZERO,
        ));
        assert_eq!(base.effective_base(), Amt::units(2));

        let mut add = ModifiedValue::new(Amt::units(10));
        add.push_modifier(modifier(
            "tax",
            SignedAmt::ZERO,
            SignedAmt::ZERO,
            SignedAmt::units(-3),
        ));
        assert_eq!(add.final_value(), Amt::units(7));

        let mut zeroed_multiplier = ModifiedValue::new(Amt::units(5));
        zeroed_multiplier.push_modifier(modifier(
            "destroy",
            SignedAmt::ZERO,
            SignedAmt::units(-2),
            SignedAmt::ZERO,
        ));
        assert_eq!(zeroed_multiplier.final_value(), Amt::ZERO);

        let mut zeroed_base = ModifiedValue::new(Amt::units(3));
        zeroed_base.push_modifier(modifier(
            "drain",
            SignedAmt::units(-10),
            SignedAmt::ZERO,
            SignedAmt::ZERO,
        ));
        assert_eq!(zeroed_base.effective_base(), Amt::ZERO);

        let mut zeroed_add = ModifiedValue::new(Amt::units(5));
        zeroed_add.push_modifier(modifier(
            "tax",
            SignedAmt::ZERO,
            SignedAmt::ZERO,
            SignedAmt::units(-10),
        ));
        assert_eq!(zeroed_add.final_value(), Amt::ZERO);
    }

    #[test]
    fn mixed_positive_and_negative_multiplier() {
        let mut value = ModifiedValue::new(Amt::units(10));
        value.push_modifier(modifier(
            "buff",
            SignedAmt::ZERO,
            SignedAmt::new(0, 500),
            SignedAmt::ZERO,
        ));
        value.push_modifier(modifier(
            "nerf",
            SignedAmt::ZERO,
            SignedAmt::new(0, -300),
            SignedAmt::ZERO,
        ));
        assert_eq!(value.total_multiplier(), SignedAmt::new(1, 200));
        assert_eq!(value.final_value(), Amt::units(12));
    }

    #[test]
    fn modified_value_drains_expired_modifiers() {
        let mut value = ModifiedValue::new(Amt::units(10));
        value.push_modifier_timed(
            modifier("timed", SignedAmt::ZERO, SignedAmt::ZERO, SignedAmt::ZERO),
            5,
            10,
        );

        assert_eq!(value.cleanup_expired(14), 0);
        assert!(value.has_modifier("timed"));
        assert_eq!(value.cleanup_expired(15), 1);
        assert!(!value.has_modifier("timed"));
    }

    #[test]
    fn permanent_modifiers_never_expire() {
        let mut value = ModifiedValue::new(Amt::units(10));
        value.push_modifier(modifier(
            "perm",
            SignedAmt::units(5),
            SignedAmt::ZERO,
            SignedAmt::ZERO,
        ));

        assert_eq!(value.cleanup_expired(0), 0);
        assert_eq!(value.cleanup_expired(1000), 0);
        assert_eq!(value.final_value(), Amt::units(15));
    }

    #[test]
    fn remaining_duration_is_clamped_to_zero() {
        let mut timed = modifier("timed", SignedAmt::ZERO, SignedAmt::ZERO, SignedAmt::ZERO);
        timed.expires_at = Some(20);

        assert_eq!(timed.remaining_duration(0), Some(20));
        assert_eq!(timed.remaining_duration(10), Some(10));
        assert_eq!(timed.remaining_duration(20), Some(0));
        assert_eq!(timed.remaining_duration(25), Some(0));
        assert_eq!(
            modifier("perm", SignedAmt::ZERO, SignedAmt::ZERO, SignedAmt::ZERO)
                .remaining_duration(100),
            None
        );
    }

    #[test]
    fn drain_expired_preserves_on_expire_event() {
        let mut value = ModifiedValue::new(Amt::units(10));
        let mut early = modifier(
            "early",
            SignedAmt::units(2),
            SignedAmt::ZERO,
            SignedAmt::ZERO,
        );
        early.expires_at = Some(5);
        early.on_expire_event = Some("early_event".to_string());
        value.push_modifier(early);

        let mut late = modifier(
            "late",
            SignedAmt::units(3),
            SignedAmt::ZERO,
            SignedAmt::ZERO,
        );
        late.expires_at = Some(15);
        value.push_modifier(late);

        let expired = value.drain_expired(10);
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].id, "early");
        assert_eq!(expired[0].on_expire_event, Some("early_event".to_string()));
        assert!(value.has_modifier("late"));
    }
}
