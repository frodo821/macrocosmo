use crate::amount::{Amt, SignedAmt};
use crate::modified_value::ModifiedValue;

/// Data-only value breakdown for UI labels with tooltip details.
#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "reflect", derive(bevy_reflect::Reflect))]
pub struct ModifiedValueDisplay {
    pub label: String,
    pub base: String,
    pub final_value: String,
    pub modifiers: Vec<ModifierDisplayLine>,
}

#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "reflect", derive(bevy_reflect::Reflect))]
pub struct ModifierDisplayLine {
    pub label: String,
    pub parts: Vec<String>,
    pub remaining_duration: Option<i64>,
}

pub fn modified_value_display(
    label: impl Into<String>,
    value: &ModifiedValue,
    format_fn: impl Fn(Amt) -> String,
    current_time: Option<i64>,
) -> ModifiedValueDisplay {
    ModifiedValueDisplay {
        label: label.into(),
        base: format_fn(value.base()),
        final_value: format_fn(value.final_value()),
        modifiers: value
            .modifiers()
            .iter()
            .map(|modifier| ModifierDisplayLine {
                label: modifier.label.clone(),
                parts: modifier_display_parts(modifier.base_add, modifier.multiplier, modifier.add),
                remaining_duration: current_time.and_then(|now| modifier.remaining_duration(now)),
            })
            .collect(),
    }
}

fn modifier_display_parts(
    base_add: SignedAmt,
    multiplier: SignedAmt,
    add: SignedAmt,
) -> Vec<String> {
    let mut parts = Vec::new();
    if base_add.raw() != 0 {
        parts.push(format!("{} (base add)", base_add.display()));
    }
    if multiplier.raw() != 0 {
        let mult_with_one = SignedAmt::units(1).add(multiplier);
        parts.push(format!("{} (mult)", display_multiplier(mult_with_one)));
    }
    if add.raw() != 0 {
        parts.push(format!("{} (add)", add.display()));
    }
    parts
}

pub fn display_multiplier(multiplier: SignedAmt) -> String {
    let abs_raw = multiplier.raw().unsigned_abs();
    let w = abs_raw / 1000;
    let f = abs_raw % 1000;
    let sign = if multiplier.raw() < 0 { "-" } else { "" };
    if f == 0 {
        format!("x{}{}", sign, w)
    } else if f % 100 == 0 {
        format!("x{}{}.{}", sign, w, f / 100)
    } else if f % 10 == 0 {
        format!("x{}{}.{:02}", sign, w, f / 10)
    } else {
        format!("x{}{}.{:03}", sign, w, f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modified_value::Modifier;

    fn modifier(
        id: &str,
        label: &str,
        base_add: SignedAmt,
        multiplier: SignedAmt,
        add: SignedAmt,
    ) -> Modifier {
        Modifier {
            id: id.to_string(),
            label: label.to_string(),
            base_add,
            multiplier,
            add,
            expires_at: Some(20),
            on_expire_event: None,
        }
    }

    #[test]
    fn builds_modified_value_display_breakdown() {
        let mut value = ModifiedValue::new(Amt::units(10));
        value.push_modifier(modifier(
            "a",
            "Tech A",
            SignedAmt::units(2),
            SignedAmt::new(0, 500),
            SignedAmt::units(1),
        ));

        assert_eq!(
            modified_value_display("Range", &value, Amt::display, Some(5)),
            ModifiedValueDisplay {
                label: "Range".to_string(),
                base: "10".to_string(),
                final_value: "19".to_string(),
                modifiers: vec![ModifierDisplayLine {
                    label: "Tech A".to_string(),
                    parts: vec![
                        "+2 (base add)".to_string(),
                        "x1.5 (mult)".to_string(),
                        "+1 (add)".to_string(),
                    ],
                    remaining_duration: Some(15),
                }],
            }
        );
    }
}
