use crate::amount::{Amt, SignedAmt};

/// A declarative modifier parsed from a Lua definition (building / job / species).
///
/// Unlike `Modifier`, this carries a *target string* used by runtime sync systems
/// to route it to the right `ModifiedValue` bucket. Target string conventions
/// (see #241):
///
/// - `colony.<field>` — colony-level aggregator (e.g. `colony.minerals_per_hexadies`)
/// - `colony.<job>_slot` — job slot capacity (e.g. `colony.miner_slot`)
/// - `job:<job_id>::<target>` — per-job rate bucket (e.g.
///   `job:miner::colony.minerals_per_hexadies`)
#[derive(Clone, Debug, PartialEq)]
pub struct ParsedModifier {
    pub target: String,
    pub base_add: f64,
    pub multiplier: f64,
    pub add: f64,
}

impl ParsedModifier {
    /// True iff this modifier targets a job-scoped bucket
    /// (`job:<id>::...`). The part after `::` is returned alongside the job id.
    pub fn job_scope(&self) -> Option<(&str, &str)> {
        let rest = self.target.strip_prefix("job:")?;
        let (job_id, target) = rest.split_once("::")?;
        Some((job_id, target))
    }

    /// Build a `Modifier` with the given id/label.
    pub fn to_modifier(&self, id: impl Into<String>, label: impl Into<String>) -> Modifier {
        Modifier {
            id: id.into(),
            label: label.into(),
            base_add: SignedAmt::from_f64(self.base_add),
            multiplier: SignedAmt::from_f64(self.multiplier),
            add: SignedAmt::from_f64(self.add),
            expires_at: None,
            on_expire_event: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Modifier {
    pub id: String,
    pub label: String,
    pub base_add: SignedAmt,
    pub multiplier: SignedAmt,
    pub add: SignedAmt,
    /// None = permanent, Some(t) = expires when clock.elapsed >= t
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
            if let Some(t) = self.modifiers[i].expires_at {
                if t <= current_time {
                    expired.push(self.modifiers.remove(i));
                    continue;
                }
            }
            i += 1;
        }
        expired
    }

    /// Remove all modifiers whose expires_at <= current_time. Returns count removed.
    pub fn cleanup_expired(&mut self, current_time: i64) -> usize {
        self.drain_expired(current_time).len()
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

/// A ModifiedValue with a generation counter for cache invalidation.
/// Generation increments on any push/pop, signaling downstream caches to recompute.
#[derive(Clone, Debug)]
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

    /// Delegate to inner ModifiedValue
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

/// Caches a computed value derived from multiple ScopedModifiers.
/// Recomputes only when any scope's generation changes.
#[derive(Clone, Debug, Default)]
pub struct CachedValue {
    cached: Amt,
    generations: Vec<u64>,
}

impl CachedValue {
    /// Get the cached value, recomputing if any scope generation changed.
    /// `scopes` should be ordered: most local first (e.g., ship, fleet, system, empire).
    /// The first scope provides the base value; subsequent scopes contribute only multipliers and adds.
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

    /// Returns true if the cache is stale (needs recomputation).
    pub fn is_stale(&self, scopes: &[&ScopedModifiers]) -> bool {
        let current_gens: Vec<u64> = scopes.iter().map(|s| s.generation()).collect();
        current_gens != self.generations
    }

    /// Read-only access to the last cached value (without recomputing).
    /// Call `get` to refresh; call this for read-only contexts (e.g. queries
    /// that can't take `&mut`).
    pub fn cached(&self) -> Amt {
        self.cached
    }

    /// Compute the combined value from multiple scopes.
    /// First scope: base + base_add from its modifiers.
    /// All scopes: multipliers and adds are combined.
    /// Formula: effective_base * (1.0 + Σ all multipliers from all scopes) + Σ all adds from all scopes
    fn compute(scopes: &[&ScopedModifiers]) -> Amt {
        if scopes.is_empty() {
            return Amt::ZERO;
        }

        // Base comes from the first (most local) scope
        let effective_base = scopes[0].effective_base();

        // Combine multipliers and adds from ALL scopes
        let mut total_mult = SignedAmt::units(1); // start at 1.0
        let mut total_add = SignedAmt::ZERO;

        for scope in scopes {
            // Each scope's total_multiplier already includes the +1.0 base,
            // so we need to subtract 1.0 to get just the modifier delta
            let scope_mult_delta = scope.total_multiplier().add(SignedAmt::units(-1));
            total_mult = total_mult.add(scope_mult_delta);
            total_add = total_add.add(scope.total_add());
        }

        // Also add base_add contributions from non-first scopes
        // (first scope's base_add is already in effective_base)
        for scope in &scopes[1..] {
            let base_add_sum: SignedAmt = scope
                .modifiers()
                .iter()
                .fold(SignedAmt::ZERO, |acc, m| acc.add(m.base_add));
            // Add to effective base via the add channel
            total_add = total_add.add(base_add_sum);
        }

        // Compute: effective_base * total_mult + total_add, clamped to 0
        // Use i128 for intermediate to avoid overflow
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

    fn make_modifier(id: &str, base_add: SignedAmt, multiplier: SignedAmt, add: SignedAmt) -> Modifier {
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

    // --- Expiration tests ---

    #[test]
    fn test_modifier_expires_at_none_is_permanent() {
        let mut mv = ModifiedValue::new(Amt::units(10));
        mv.push_modifier(make_modifier("perm", SignedAmt::units(5), SignedAmt::ZERO, SignedAmt::ZERO));
        // cleanup at any time should not remove permanent modifiers
        assert_eq!(mv.cleanup_expired(0), 0);
        assert_eq!(mv.cleanup_expired(1000), 0);
        assert_eq!(mv.modifiers().len(), 1);
        assert_eq!(mv.final_value(), Amt::units(15));
    }

    #[test]
    fn test_modifier_expires_after_duration() {
        let mut mv = ModifiedValue::new(Amt::units(10));
        let m = make_modifier("timed", SignedAmt::units(5), SignedAmt::ZERO, SignedAmt::ZERO);
        mv.push_modifier_timed(m, 0, 10); // expires_at = 10

        // At clock=9, still present
        assert_eq!(mv.cleanup_expired(9), 0);
        assert_eq!(mv.modifiers().len(), 1);
        assert_eq!(mv.final_value(), Amt::units(15));

        // At clock=10, removed (expires_at <= current_time)
        assert_eq!(mv.cleanup_expired(10), 1);
        assert_eq!(mv.modifiers().len(), 0);
        assert_eq!(mv.final_value(), Amt::units(10));
    }

    #[test]
    fn test_cleanup_removes_only_expired() {
        let mut mv = ModifiedValue::new(Amt::units(10));

        // Permanent modifier
        mv.push_modifier(make_modifier("perm", SignedAmt::units(1), SignedAmt::ZERO, SignedAmt::ZERO));

        // Expires at 5
        let m1 = make_modifier("early", SignedAmt::units(2), SignedAmt::ZERO, SignedAmt::ZERO);
        mv.push_modifier_timed(m1, 0, 5);

        // Expires at 15
        let m2 = make_modifier("late", SignedAmt::units(3), SignedAmt::ZERO, SignedAmt::ZERO);
        mv.push_modifier_timed(m2, 0, 15);

        assert_eq!(mv.modifiers().len(), 3);

        // At clock=10, only "early" (expires_at=5) is removed
        assert_eq!(mv.cleanup_expired(10), 1);
        assert_eq!(mv.modifiers().len(), 2);
        assert!(mv.has_modifier("perm"));
        assert!(!mv.has_modifier("early"));
        assert!(mv.has_modifier("late"));
    }

    #[test]
    fn test_remaining_duration() {
        let m = Modifier {
            id: "test".to_string(),
            label: "Test".to_string(),
            base_add: SignedAmt::ZERO,
            multiplier: SignedAmt::ZERO,
            add: SignedAmt::ZERO,
            expires_at: Some(20),
            on_expire_event: None,
        };

        assert_eq!(m.remaining_duration(0), Some(20));
        assert_eq!(m.remaining_duration(10), Some(10));
        assert_eq!(m.remaining_duration(20), Some(0));
        assert_eq!(m.remaining_duration(25), Some(0)); // clamped to 0

        // Permanent modifier
        let perm = make_modifier("perm", SignedAmt::ZERO, SignedAmt::ZERO, SignedAmt::ZERO);
        assert_eq!(perm.remaining_duration(100), None);
    }

    #[test]
    fn test_on_expire_event_field() {
        let mut mv = ModifiedValue::new(Amt::units(10));
        let m = Modifier {
            id: "timed".to_string(),
            label: "Timed".to_string(),
            base_add: SignedAmt::units(5),
            multiplier: SignedAmt::ZERO,
            add: SignedAmt::ZERO,
            expires_at: Some(10),
            on_expire_event: Some("test_event".to_string()),
        };
        mv.push_modifier(m);
        assert_eq!(mv.modifiers().len(), 1);
        assert_eq!(mv.modifiers()[0].on_expire_event, Some("test_event".to_string()));

        // drain should return the modifier with on_expire_event preserved
        let expired = mv.drain_expired(10);
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].id, "timed");
        assert_eq!(expired[0].on_expire_event, Some("test_event".to_string()));
        assert_eq!(mv.modifiers().len(), 0);
    }

    #[test]
    fn test_drain_expired_returns_modifiers() {
        let mut mv = ModifiedValue::new(Amt::units(10));

        // Permanent modifier
        mv.push_modifier(make_modifier("perm", SignedAmt::units(1), SignedAmt::ZERO, SignedAmt::ZERO));

        // Expires at 5, with on_expire_event
        let m1 = Modifier {
            id: "early".to_string(),
            label: "Early".to_string(),
            base_add: SignedAmt::units(2),
            multiplier: SignedAmt::ZERO,
            add: SignedAmt::ZERO,
            expires_at: Some(5),
            on_expire_event: Some("early_event".to_string()),
        };
        mv.push_modifier(m1);

        // Expires at 15, no event
        let m2 = Modifier {
            id: "late".to_string(),
            label: "Late".to_string(),
            base_add: SignedAmt::units(3),
            multiplier: SignedAmt::ZERO,
            add: SignedAmt::ZERO,
            expires_at: Some(15),
            on_expire_event: None,
        };
        mv.push_modifier(m2);

        assert_eq!(mv.modifiers().len(), 3);

        // Drain at clock=10: only "early" (expires_at=5) is removed
        let expired = mv.drain_expired(10);
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].id, "early");
        assert_eq!(expired[0].on_expire_event, Some("early_event".to_string()));

        assert_eq!(mv.modifiers().len(), 2);
        assert!(mv.has_modifier("perm"));
        assert!(mv.has_modifier("late"));
    }

    // --- ScopedModifiers tests ---

    #[test]
    fn test_scoped_modifiers_generation_increments() {
        let mut sm = ScopedModifiers::new(Amt::units(10));
        assert_eq!(sm.generation(), 0);

        sm.push_modifier(make_modifier("a", SignedAmt::units(1), SignedAmt::ZERO, SignedAmt::ZERO));
        assert_eq!(sm.generation(), 1);

        sm.push_modifier(make_modifier("b", SignedAmt::units(2), SignedAmt::ZERO, SignedAmt::ZERO));
        assert_eq!(sm.generation(), 2);

        sm.pop_modifier("a");
        assert_eq!(sm.generation(), 3);

        // Popping non-existent does not increment
        sm.pop_modifier("nonexistent");
        assert_eq!(sm.generation(), 3);
    }

    #[test]
    fn test_scoped_modifiers_base_change_increments() {
        let mut sm = ScopedModifiers::new(Amt::units(10));
        assert_eq!(sm.generation(), 0);

        sm.set_base(Amt::units(20));
        assert_eq!(sm.generation(), 1);
    }

    #[test]
    fn test_scoped_modifiers_same_base_no_increment() {
        let mut sm = ScopedModifiers::new(Amt::units(10));
        assert_eq!(sm.generation(), 0);

        sm.set_base(Amt::units(10));
        assert_eq!(sm.generation(), 0);
    }

    #[test]
    fn test_cached_value_recomputes_on_generation_change() {
        let mut scope = ScopedModifiers::new(Amt::units(10));
        let mut cache = CachedValue::default();

        let val = cache.get(&[&scope]);
        assert_eq!(val, Amt::units(10));

        scope.push_modifier(make_modifier("buff", SignedAmt::ZERO, SignedAmt::new(0, 500), SignedAmt::ZERO));
        // +50% multiplier: 10 * 1.5 = 15
        let val = cache.get(&[&scope]);
        assert_eq!(val, Amt::units(15));
    }

    #[test]
    fn test_cached_value_cache_hit() {
        let scope = ScopedModifiers::new(Amt::units(10));
        let mut cache = CachedValue::default();

        let _ = cache.get(&[&scope]);
        assert!(!cache.is_stale(&[&scope]));

        // Getting again without changes should not be stale
        let _ = cache.get(&[&scope]);
        assert!(!cache.is_stale(&[&scope]));
    }

    #[test]
    fn test_cached_value_multi_scope() {
        // Ship base=10, system mult=+20%, empire mult=+10%
        // Expected: 10 * (1.0 + 0.2 + 0.1) = 10 * 1.3 = 13
        let ship = ScopedModifiers::new(Amt::units(10));
        let mut system = ScopedModifiers::default();
        system.push_modifier(make_modifier("sys_buff", SignedAmt::ZERO, SignedAmt::new(0, 200), SignedAmt::ZERO));
        let mut empire = ScopedModifiers::default();
        empire.push_modifier(make_modifier("emp_buff", SignedAmt::ZERO, SignedAmt::new(0, 100), SignedAmt::ZERO));

        let mut cache = CachedValue::default();
        let val = cache.get(&[&ship, &system, &empire]);
        assert_eq!(val, Amt::units(13));
    }

    #[test]
    fn test_cached_value_one_scope_changes() {
        let ship = ScopedModifiers::new(Amt::units(10));
        let mut system = ScopedModifiers::default();
        let mut cache = CachedValue::default();

        let val = cache.get(&[&ship, &system]);
        assert_eq!(val, Amt::units(10));
        assert!(!cache.is_stale(&[&ship, &system]));

        // Change only system scope
        system.push_modifier(make_modifier("sys_buff", SignedAmt::ZERO, SignedAmt::new(0, 200), SignedAmt::ZERO));
        assert!(cache.is_stale(&[&ship, &system]));

        let val = cache.get(&[&ship, &system]);
        assert_eq!(val, Amt::units(12)); // 10 * 1.2 = 12
    }

    #[test]
    fn test_cached_value_empty_scopes() {
        let mut cache = CachedValue::default();
        let val = cache.get(&[]);
        assert_eq!(val, Amt::ZERO);
    }
}
