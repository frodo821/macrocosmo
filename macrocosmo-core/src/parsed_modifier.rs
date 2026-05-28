use crate::amount::SignedAmt;
use crate::modified_value::Modifier;

/// A declarative modifier parsed from authored definitions.
///
/// Unlike [`Modifier`], this carries a target string used by runtime sync
/// systems to route it to the right [`crate::ModifiedValue`] bucket.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "reflect", derive(bevy_reflect::Reflect))]
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

    /// Parse `docked_to:<filter>::<target>` syntax.
    ///
    /// `filter` is one of `"self"`, `"*"`, or a host-defined hull id.
    pub fn docked_scope(&self) -> Option<(&str, &str)> {
        let rest = self.target.strip_prefix("docked_to:")?;
        let (filter, target) = rest.split_once("::")?;
        if filter.is_empty() || target.is_empty() {
            return None;
        }
        Some((filter, target))
    }

    /// Build a runtime [`Modifier`] with the given id/label.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(target: &str) -> ParsedModifier {
        ParsedModifier {
            target: target.to_string(),
            base_add: 1.0,
            multiplier: 0.0,
            add: 0.0,
        }
    }

    #[test]
    fn job_scope_extracts_job_and_target() {
        let pm = parsed("job:miner::colony.minerals_per_hexadies");
        assert_eq!(
            pm.job_scope(),
            Some(("miner", "colony.minerals_per_hexadies"))
        );
        assert_eq!(parsed("colony.minerals_per_hexadies").job_scope(), None);
    }

    #[test]
    fn to_modifier_converts_numeric_channels() {
        let pm = ParsedModifier {
            target: "ship.speed".to_string(),
            base_add: 1.25,
            multiplier: -0.2,
            add: 3.5,
        };

        let modifier = pm.to_modifier("speed_mod", "Speed Mod");
        assert_eq!(modifier.id, "speed_mod");
        assert_eq!(modifier.label, "Speed Mod");
        assert_eq!(modifier.base_add, SignedAmt::from_f64(1.25));
        assert_eq!(modifier.multiplier, SignedAmt::from_f64(-0.2));
        assert_eq!(modifier.add, SignedAmt::from_f64(3.5));
        assert_eq!(modifier.expires_at, None);
        assert_eq!(modifier.on_expire_event, None);
    }

    #[test]
    fn docked_scope_self() {
        let pm = parsed("docked_to:self::ship.speed");
        assert_eq!(pm.docked_scope(), Some(("self", "ship.speed")));
    }

    #[test]
    fn docked_scope_hull_id() {
        let pm = parsed("docked_to:carrier_hull::ship.shield_regen");
        assert_eq!(
            pm.docked_scope(),
            Some(("carrier_hull", "ship.shield_regen"))
        );
    }

    #[test]
    fn docked_scope_wildcard() {
        let pm = parsed("docked_to:*::ship.evasion");
        assert_eq!(pm.docked_scope(), Some(("*", "ship.evasion")));
    }

    #[test]
    fn docked_scope_rejects_invalid_shapes() {
        assert_eq!(parsed("ship.speed").docked_scope(), None);
        assert_eq!(parsed("docked_to:::ship.speed").docked_scope(), None);
        assert_eq!(parsed("docked_to:self:ship.speed").docked_scope(), None);
        assert_eq!(
            parsed("job:miner::colony.minerals_per_hexadies").docked_scope(),
            None
        );
    }
}
