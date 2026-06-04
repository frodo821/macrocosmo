use std::collections::HashMap;

/// Descriptor effect asking the UI runtime to present a matching UI fragment.
///
/// This is intentionally data-only. Hosts own fragment matching, constraints,
/// and rendering.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "reflect", derive(bevy_reflect::Reflect))]
pub struct UiFragmentPresentationRequest {
    pub context: HashMap<String, String>,
    pub labels_all: Vec<String>,
    pub labels_any: Vec<String>,
    pub preferred_host: Option<String>,
    pub mode: Option<String>,
}

/// A UI-displayable effect command. Lua callbacks return or accumulate these
/// descriptors; host crates decide how to apply them.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "reflect", derive(bevy_reflect::Reflect))]
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
    PresentUiFragment {
        request: UiFragmentPresentationRequest,
    },
    Hidden {
        label: String,
        #[cfg_attr(feature = "reflect", reflect(ignore, default = "default_hidden_inner"))]
        inner: Box<DescriptiveEffect>,
    },
}

/// Default for `Hidden.inner` when reflected reconstruction needs one.
pub fn default_hidden_inner() -> Box<DescriptiveEffect> {
    Box::new(DescriptiveEffect::SetFlag {
        name: String::new(),
        value: false,
        description: None,
    })
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
                let target_display = format_target(target);
                if parts.is_empty() {
                    format!("Modify: {target_display}")
                } else {
                    format!("{target_display} {}", parts.join(", "))
                }
            }
            DescriptiveEffect::PopModifier { target } => {
                format!("Removes: {}", format_target(target))
            }
            DescriptiveEffect::SetFlag {
                name, description, ..
            } => description.clone().unwrap_or_else(|| name.clone()),
            DescriptiveEffect::FireEvent { event_id, .. } => {
                format!("Triggers: {event_id}")
            }
            DescriptiveEffect::PresentUiFragment { request } => {
                let labels = if request.labels_all.is_empty() {
                    request.labels_any.join(", ")
                } else {
                    request.labels_all.join(", ")
                };
                if labels.is_empty() {
                    "Presents UI fragment".to_string()
                } else {
                    format!("Presents UI fragment: {labels}")
                }
            }
            DescriptiveEffect::Hidden { label, .. } => label.clone(),
        }
    }
}

fn format_target(target: &str) -> String {
    let parts: Vec<&str> = target.split('.').collect();
    if parts.len() == 2 {
        let mut second = parts[1].to_string();
        if let Some(first_char) = second.get_mut(..1) {
            first_char.make_ascii_uppercase();
        }
        format!("{second} {}", parts[0])
    } else {
        let mut s = target.to_string();
        if let Some(first_char) = s.get_mut(..1) {
            first_char.make_ascii_uppercase();
        }
        s
    }
}
