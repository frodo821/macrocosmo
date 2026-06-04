use crate::amount::{Amt, SignedAmt};
use crate::modified_value::{ModifiedValue, Modifier};

/// A [`ModifiedValue`] with a generation counter for cache invalidation.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "reflect", derive(bevy_reflect::Reflect))]
pub struct ScopedModifiers {
    value: ModifiedValue,
    generation: u64,
}

impl Default for ScopedModifiers {
    fn default() -> Self {
        Self {
            value: ModifiedValue::default(),
            generation: 0,
        }
    }
}

impl ScopedModifiers {
    pub fn new(base: Amt) -> Self {
        Self {
            value: ModifiedValue::new(base),
            generation: 0,
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn value(&self) -> &ModifiedValue {
        &self.value
    }

    pub fn value_mut(&mut self) -> &mut ModifiedValue {
        &mut self.value
    }

    pub fn push_modifier(&mut self, modifier: Modifier) {
        self.value.push_modifier(modifier);
        self.generation += 1;
    }

    pub fn pop_modifier(&mut self, id: &str) -> Option<Modifier> {
        let result = self.value.pop_modifier(id);
        if result.is_some() {
            self.generation += 1;
        }
        result
    }

    pub fn set_base(&mut self, base: Amt) {
        let old = self.value.base();
        self.value.set_base(base);
        if old != base {
            self.generation += 1;
        }
    }

    pub fn final_value(&self) -> Amt {
        self.value.final_value()
    }

    pub fn effective_base(&self) -> Amt {
        self.value.effective_base()
    }

    pub fn total_multiplier(&self) -> SignedAmt {
        self.value.total_multiplier()
    }

    pub fn total_add(&self) -> SignedAmt {
        self.value.total_add()
    }

    pub fn modifiers(&self) -> &[Modifier] {
        self.value.modifiers()
    }

    pub fn cleanup_expired(&mut self, current_time: i64) -> usize {
        let count = self.value.cleanup_expired(current_time);
        if count > 0 {
            self.generation += 1;
        }
        count
    }

    pub fn drain_expired(&mut self, current_time: i64) -> Vec<Modifier> {
        let expired = self.value.drain_expired(current_time);
        if !expired.is_empty() {
            self.generation += 1;
        }
        expired
    }
}

/// Caches a computed value derived from multiple [`ScopedModifiers`].
#[derive(Clone, Debug, Default)]
#[cfg_attr(feature = "reflect", derive(bevy_reflect::Reflect))]
pub struct CachedValue {
    cached: Amt,
    generations: Vec<u64>,
}

impl CachedValue {
    /// Get the cached value, recomputing if any scope generation changed.
    pub fn get(&mut self, scopes: &[&ScopedModifiers]) -> Amt {
        let current_gens: Vec<u64> = scopes.iter().map(|s| s.generation()).collect();
        if current_gens != self.generations {
            self.cached = Self::compute(scopes);
            self.generations = current_gens;
        }
        self.cached
    }

    /// Force recomputation regardless of generation.
    pub fn recompute(&mut self, scopes: &[&ScopedModifiers]) -> Amt {
        self.cached = Self::compute(scopes);
        self.generations = scopes.iter().map(|s| s.generation()).collect();
        self.cached
    }

    pub fn is_stale(&self, scopes: &[&ScopedModifiers]) -> bool {
        let current_gens: Vec<u64> = scopes.iter().map(|s| s.generation()).collect();
        current_gens != self.generations
    }

    pub fn cached(&self) -> Amt {
        self.cached
    }

    fn compute(scopes: &[&ScopedModifiers]) -> Amt {
        if scopes.is_empty() {
            return Amt::ZERO;
        }

        let effective_base = scopes[0].effective_base();
        let mut total_mult = SignedAmt::units(1);
        let mut total_add = SignedAmt::ZERO;

        for scope in scopes {
            let scope_mult_delta = scope.total_multiplier().add(SignedAmt::units(-1));
            total_mult = total_mult.add(scope_mult_delta);
            total_add = total_add.add(scope.total_add());
        }

        for scope in &scopes[1..] {
            let base_add_sum: SignedAmt = scope
                .modifiers()
                .iter()
                .fold(SignedAmt::ZERO, |acc, m| acc.add(m.base_add));
            total_add = total_add.add(base_add_sum);
        }

        let eb = effective_base.raw() as i128;
        let mult = total_mult.raw().max(0) as i128;
        let intermediate = eb * mult / 1000;
        let result = intermediate + total_add.raw() as i128;

        if result <= 0 {
            Amt::ZERO
        } else if result > u64::MAX as i128 {
            Amt(u64::MAX)
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
    fn scoped_modifiers_generation_increments() {
        let mut sm = ScopedModifiers::new(Amt::units(10));
        assert_eq!(sm.generation(), 0);

        sm.push_modifier(modifier(
            "a",
            SignedAmt::units(1),
            SignedAmt::ZERO,
            SignedAmt::ZERO,
        ));
        assert_eq!(sm.generation(), 1);

        sm.push_modifier(modifier(
            "b",
            SignedAmt::units(2),
            SignedAmt::ZERO,
            SignedAmt::ZERO,
        ));
        assert_eq!(sm.generation(), 2);

        sm.pop_modifier("a");
        assert_eq!(sm.generation(), 3);

        sm.pop_modifier("nonexistent");
        assert_eq!(sm.generation(), 3);
    }

    #[test]
    fn scoped_modifiers_base_change_increments_only_when_changed() {
        let mut sm = ScopedModifiers::new(Amt::units(10));
        sm.set_base(Amt::units(10));
        assert_eq!(sm.generation(), 0);

        sm.set_base(Amt::units(20));
        assert_eq!(sm.generation(), 1);
    }

    #[test]
    fn scoped_modifiers_expiration_increments_generation_only_when_removed() {
        let mut sm = ScopedModifiers::new(Amt::units(10));
        sm.value_mut().push_modifier_timed(
            modifier(
                "timed",
                SignedAmt::units(1),
                SignedAmt::ZERO,
                SignedAmt::ZERO,
            ),
            0,
            10,
        );
        assert_eq!(sm.generation(), 0);

        assert_eq!(sm.cleanup_expired(9), 0);
        assert_eq!(sm.generation(), 0);
        assert_eq!(sm.cleanup_expired(10), 1);
        assert_eq!(sm.generation(), 1);
    }

    #[test]
    fn cached_value_recomputes_on_generation_change() {
        let mut scope = ScopedModifiers::new(Amt::units(10));
        let mut cache = CachedValue::default();

        assert_eq!(cache.get(&[&scope]), Amt::units(10));

        scope.push_modifier(modifier(
            "buff",
            SignedAmt::ZERO,
            SignedAmt::new(0, 500),
            SignedAmt::ZERO,
        ));
        assert!(cache.is_stale(&[&scope]));
        assert_eq!(cache.get(&[&scope]), Amt::units(15));
        assert_eq!(cache.cached(), Amt::units(15));
    }

    #[test]
    fn cached_value_cache_hit() {
        let scope = ScopedModifiers::new(Amt::units(10));
        let mut cache = CachedValue::default();

        let _ = cache.get(&[&scope]);
        assert!(!cache.is_stale(&[&scope]));
        let _ = cache.get(&[&scope]);
        assert!(!cache.is_stale(&[&scope]));
    }

    #[test]
    fn cached_value_multi_scope() {
        let ship = ScopedModifiers::new(Amt::units(10));
        let mut system = ScopedModifiers::default();
        system.push_modifier(modifier(
            "sys_buff",
            SignedAmt::ZERO,
            SignedAmt::new(0, 200),
            SignedAmt::ZERO,
        ));
        let mut empire = ScopedModifiers::default();
        empire.push_modifier(modifier(
            "emp_buff",
            SignedAmt::ZERO,
            SignedAmt::new(0, 100),
            SignedAmt::ZERO,
        ));

        let mut cache = CachedValue::default();
        assert_eq!(cache.get(&[&ship, &system, &empire]), Amt::units(13));
    }

    #[test]
    fn cached_value_one_scope_changes() {
        let ship = ScopedModifiers::new(Amt::units(10));
        let mut system = ScopedModifiers::default();
        let mut cache = CachedValue::default();

        assert_eq!(cache.get(&[&ship, &system]), Amt::units(10));
        assert!(!cache.is_stale(&[&ship, &system]));

        system.push_modifier(modifier(
            "sys_buff",
            SignedAmt::ZERO,
            SignedAmt::new(0, 200),
            SignedAmt::ZERO,
        ));
        assert!(cache.is_stale(&[&ship, &system]));
        assert_eq!(cache.get(&[&ship, &system]), Amt::units(12));
    }

    #[test]
    fn cached_value_empty_scopes() {
        let mut cache = CachedValue::default();
        assert_eq!(cache.get(&[]), Amt::ZERO);
    }
}
