use std::collections::HashMap;

/// A UI-displayable effect command. Returned by Lua callbacks (on_researched, on_chosen, etc.)
#[derive(Clone, Debug)]
pub enum DescriptiveEffect {
    PushModifier {
        target: String,
        base_add: f64,
        multiplier: f64,
        add: f64,
        description: Option<String>,
    },
    PopModifier {
        target: String,
    },
    SetFlag {
        name: String,
        value: bool,
        description: Option<String>,
    },
    FireEvent {
        event_id: String,
        payload: HashMap<String, String>,
    },
    Hidden {
        label: String,
        inner: Box<DescriptiveEffect>,
    },
}

impl DescriptiveEffect {
    /// Human-readable description for UI display.
    pub fn display_text(&self) -> String {
        match self {
            DescriptiveEffect::PushModifier {
                target,
                base_add,
                multiplier,
                add,
                description,
            } => {
                if let Some(desc) = description {
                    return desc.clone();
                }
                // Build a human-readable summary from the numeric fields
                let mut parts = Vec::new();
                if *base_add != 0.0 {
                    let sign = if *base_add > 0.0 { "+" } else { "" };
                    parts.push(format!("{sign}{base_add:.0} base"));
                }
                if *multiplier != 0.0 {
                    let pct = multiplier * 100.0;
                    let sign = if pct > 0.0 { "+" } else { "" };
                    parts.push(format!("{sign}{pct:.0}%"));
                }
                if *add != 0.0 {
                    let sign = if *add > 0.0 { "+" } else { "" };
                    parts.push(format!("{sign}{add:.0} flat"));
                }
                // Format target for display: "production.minerals" -> "Mineral production"
                let target_display = format_target(target);
                if parts.is_empty() {
                    format!("Modify: {target_display}")
                } else {
                    format!("{target_display} {}", parts.join(", "))
                }
            }
            DescriptiveEffect::PopModifier { target } => {
                let target_display = format_target(target);
                format!("Removes: {target_display}")
            }
            DescriptiveEffect::SetFlag {
                name,
                value: _,
                description,
            } => {
                if let Some(desc) = description {
                    desc.clone()
                } else {
                    name.clone()
                }
            }
            DescriptiveEffect::FireEvent {
                event_id,
                payload: _,
            } => {
                format!("Triggers: {event_id}")
            }
            DescriptiveEffect::Hidden { label, inner: _ } => label.clone(),
        }
    }
}

/// Format a dotted target key into a human-readable string.
/// e.g. "production.minerals" -> "Mineral production"
///      "ship.speed" -> "Ship speed"
fn format_target(target: &str) -> String {
    let parts: Vec<&str> = target.split('.').collect();
    if parts.len() == 2 {
        // Capitalize the second part and put it first: "production.minerals" -> "Mineral production"
        let mut second = parts[1].to_string();
        if let Some(first_char) = second.get_mut(..1) {
            first_char.make_ascii_uppercase();
        }
        format!("{second} {}", parts[0])
    } else {
        // Fallback: just capitalize the whole thing
        let mut s = target.to_string();
        if let Some(first_char) = s.get_mut(..1) {
            first_char.make_ascii_uppercase();
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_display_text_push_modifier_with_description() {
        let effect = DescriptiveEffect::PushModifier {
            target: "production.minerals".into(),
            base_add: 0.0,
            multiplier: 0.15,
            add: 0.0,
            description: Some("Mineral production +15%".into()),
        };
        assert_eq!(effect.display_text(), "Mineral production +15%");
    }

    #[test]
    fn test_display_text_push_modifier_multiplier_only() {
        let effect = DescriptiveEffect::PushModifier {
            target: "production.minerals".into(),
            base_add: 0.0,
            multiplier: 0.15,
            add: 0.0,
            description: None,
        };
        assert_eq!(effect.display_text(), "Minerals production +15%");
    }

    #[test]
    fn test_display_text_push_modifier_base_add() {
        let effect = DescriptiveEffect::PushModifier {
            target: "ship.speed".into(),
            base_add: 5.0,
            multiplier: 0.0,
            add: 0.0,
            description: None,
        };
        assert_eq!(effect.display_text(), "Speed ship +5 base");
    }

    #[test]
    fn test_display_text_push_modifier_combined() {
        let effect = DescriptiveEffect::PushModifier {
            target: "production.energy".into(),
            base_add: 2.0,
            multiplier: 0.1,
            add: 1.0,
            description: None,
        };
        assert_eq!(
            effect.display_text(),
            "Energy production +2 base, +10%, +1 flat"
        );
    }

    #[test]
    fn test_display_text_pop_modifier() {
        let effect = DescriptiveEffect::PopModifier {
            target: "production.minerals".into(),
        };
        assert_eq!(effect.display_text(), "Removes: Minerals production");
    }

    #[test]
    fn test_display_text_set_flag_with_description() {
        let effect = DescriptiveEffect::SetFlag {
            name: "automated_mining".into(),
            value: true,
            description: Some("Enables automated mining".into()),
        };
        assert_eq!(effect.display_text(), "Enables automated mining");
    }

    #[test]
    fn test_display_text_set_flag_without_description() {
        let effect = DescriptiveEffect::SetFlag {
            name: "automated_mining".into(),
            value: true,
            description: None,
        };
        assert_eq!(effect.display_text(), "automated_mining");
    }

    #[test]
    fn test_display_text_fire_event() {
        let effect = DescriptiveEffect::FireEvent {
            event_id: "first_contact".into(),
            payload: HashMap::new(),
        };
        assert_eq!(effect.display_text(), "Triggers: first_contact");
    }

    #[test]
    fn test_display_text_hidden() {
        let inner = DescriptiveEffect::SetFlag {
            name: "secret".into(),
            value: true,
            description: None,
        };
        let effect = DescriptiveEffect::Hidden {
            label: "Something mysterious happens...".into(),
            inner: Box::new(inner),
        };
        assert_eq!(effect.display_text(), "Something mysterious happens...");
    }

    #[test]
    fn test_display_text_negative_multiplier() {
        let effect = DescriptiveEffect::PushModifier {
            target: "ship.speed".into(),
            base_add: 0.0,
            multiplier: -0.2,
            add: 0.0,
            description: None,
        };
        assert_eq!(effect.display_text(), "Speed ship -20%");
    }

    #[test]
    fn test_format_target_single_part() {
        assert_eq!(format_target("speed"), "Speed");
    }

    #[test]
    fn test_format_target_dotted() {
        assert_eq!(format_target("production.minerals"), "Minerals production");
    }
}
