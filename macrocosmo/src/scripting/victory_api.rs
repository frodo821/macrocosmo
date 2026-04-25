//! Lua-table parser for [`macrocosmo_ai::VictoryCondition`] and the
//! supporting [`macrocosmo_ai::Condition`] tree.
//!
//! This is the **AI**-side condition parser. Do not confuse it with
//! [`crate::scripting::condition_parser`], which targets the
//! game-side `crate::condition::Condition` tree (different variants,
//! scoped flags, etc.). The two parsers are intentionally kept separate
//! because the trees are structurally distinct.
//!
//! # v1 surface
//!
//! ```lua
//! {
//!   win          = { kind = "metric_above", metric = "colony_count.faction_1", threshold = 1.0 },
//!   prerequisites = { kind = "all", children = {} },  -- optional
//!   time_limit   = nil,                                -- optional integer Tick
//!   -- score_hint: ignored in v1
//! }
//! ```
//!
//! Supported `kind` values for the `Condition` tree:
//!
//! | `kind`           | Maps to                                                              |
//! |------------------|----------------------------------------------------------------------|
//! | `"always"`       | `Condition::Always`                                                  |
//! | `"never"`        | `Condition::Never`                                                   |
//! | `"all"`          | `Condition::All(children)`                                           |
//! | `"any"`          | `Condition::Any(children)`                                           |
//! | `"one_of"`       | `Condition::OneOf(children)`                                         |
//! | `"not"`          | `Condition::Not(child)`                                              |
//! | `"metric_above"` | `Condition::Atom(ConditionAtom::MetricAbove { metric, threshold })`  |
//! | `"metric_below"` | `Condition::Atom(ConditionAtom::MetricBelow { metric, threshold })`  |
//! | `"metric_present"` | `Condition::Atom(ConditionAtom::MetricPresent { metric })`         |
//!
//! Other [`macrocosmo_ai::ConditionAtom`] variants (`Compare`,
//! `ValueMissing`, `MetricStale`, `EvidenceCountExceeds`,
//! `EvidenceRateAbove`, `Standing*`) carry structured / typed parameters
//! (`ValueExpr`, `EvidenceKindId`, `FactionId`, ...) that are not yet
//! exposed through this thin v1 surface; calling with one of those
//! `kind` values returns a clear "unsupported in v1" error so future
//! extensions are obvious.
//!
//! `Condition::All { children: vec![] }` ("vacuous true") is the
//! default for a missing `prerequisites` field.

use macrocosmo_ai::{Condition, ConditionAtom, MetricId, Tick, VictoryCondition};

/// Parse a Lua table into a [`VictoryCondition`].
///
/// See module docs for the supported shape.
pub fn parse_ai_victory_condition(
    _lua: &mlua::Lua,
    table: mlua::Table,
) -> mlua::Result<VictoryCondition> {
    let win_value: mlua::Value = table.get("win")?;
    let win_table = match win_value {
        mlua::Value::Table(t) => t,
        _ => {
            return Err(mlua::Error::external(
                "VictoryCondition: `win` must be a condition table".to_string(),
            ));
        }
    };
    let win = parse_ai_condition(&win_table)?;

    let prereq_value: mlua::Value = table.get("prerequisites")?;
    let prerequisites = match prereq_value {
        mlua::Value::Nil => Condition::All(Vec::new()), // vacuous true
        mlua::Value::Table(t) => parse_ai_condition(&t)?,
        _ => {
            return Err(mlua::Error::external(
                "VictoryCondition: `prerequisites` must be a condition table or nil".to_string(),
            ));
        }
    };

    let time_limit_value: mlua::Value = table.get("time_limit")?;
    let time_limit: Option<Tick> = match time_limit_value {
        mlua::Value::Nil => None,
        mlua::Value::Integer(i) => Some(i as Tick),
        mlua::Value::Number(n) => Some(n as Tick),
        _ => {
            return Err(mlua::Error::external(
                "VictoryCondition: `time_limit` must be nil or an integer".to_string(),
            ));
        }
    };

    // `score_hint` deliberately ignored in v1 (see module docs).

    Ok(VictoryCondition {
        win,
        prerequisites,
        time_limit,
        score_hint: None,
    })
}

/// Parse a Lua condition table into a [`macrocosmo_ai::Condition`] tree.
///
/// Recursive — `all` / `any` / `one_of` traverse `children` (a Lua
/// sequence of nested tables); `not` traverses `child`.
pub fn parse_ai_condition(table: &mlua::Table) -> mlua::Result<Condition> {
    let kind: String = table.get("kind").map_err(|e| {
        mlua::Error::external(format!(
            "ai-condition: missing `kind` field on condition table: {e}"
        ))
    })?;

    match kind.as_str() {
        "always" => Ok(Condition::Always),
        "never" => Ok(Condition::Never),

        "all" => {
            let children = parse_children(table)?;
            Ok(Condition::All(children))
        }
        "any" => {
            let children = parse_children(table)?;
            Ok(Condition::Any(children))
        }
        "one_of" => {
            let children = parse_children(table)?;
            Ok(Condition::OneOf(children))
        }
        "not" => {
            let child_value: mlua::Value = table.get("child")?;
            let child_table = match child_value {
                mlua::Value::Table(t) => t,
                _ => {
                    return Err(mlua::Error::external(
                        "ai-condition: `not` requires a `child` condition table".to_string(),
                    ));
                }
            };
            let child = parse_ai_condition(&child_table)?;
            Ok(Condition::Not(Box::new(child)))
        }

        "metric_above" => {
            let metric: String = table.get("metric")?;
            let threshold: f64 = table.get("threshold")?;
            Ok(Condition::Atom(ConditionAtom::MetricAbove {
                metric: MetricId::from(metric),
                threshold,
            }))
        }
        "metric_below" => {
            let metric: String = table.get("metric")?;
            let threshold: f64 = table.get("threshold")?;
            Ok(Condition::Atom(ConditionAtom::MetricBelow {
                metric: MetricId::from(metric),
                threshold,
            }))
        }
        "metric_present" => {
            let metric: String = table.get("metric")?;
            Ok(Condition::Atom(ConditionAtom::MetricPresent {
                metric: MetricId::from(metric),
            }))
        }

        // The remaining ConditionAtom variants take structured payloads
        // (ValueExpr, EvidenceKindId, FactionId, ...) that have no v1
        // Lua surface yet. Surface a clear error so upstream knows to
        // file an extension request rather than silently dropping the
        // condition.
        unsupported @ ("compare"
        | "value_missing"
        | "metric_stale"
        | "evidence_count_exceeds"
        | "evidence_rate_above"
        | "standing_below"
        | "standing_above"
        | "standing_confidence_above") => Err(mlua::Error::external(format!(
            "ai-condition: `{unsupported}` is unsupported in v1; \
                 file an extension request to widen the Lua surface"
        ))),

        other => Err(mlua::Error::external(format!(
            "ai-condition: unknown `kind` value: `{other}`"
        ))),
    }
}

/// Read the `children` field of a combinator table as a Lua sequence
/// of nested condition tables and parse each in turn.
fn parse_children(table: &mlua::Table) -> mlua::Result<Vec<Condition>> {
    let children_value: mlua::Value = table.get("children")?;
    let children_table = match children_value {
        mlua::Value::Nil => return Ok(Vec::new()),
        mlua::Value::Table(t) => t,
        _ => {
            return Err(mlua::Error::external(
                "ai-condition: combinator `children` must be a sequence table".to_string(),
            ));
        }
    };
    children_table
        .sequence_values::<mlua::Table>()
        .map(|res| {
            let t = res?;
            parse_ai_condition(&t)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mlua::Lua;

    /// Helper: build a condition Lua table from a closure that mutates
    /// a fresh table.
    fn cond_table(lua: &Lua, build: impl FnOnce(&mlua::Table)) -> mlua::Table {
        let t = lua.create_table().unwrap();
        build(&t);
        t
    }

    #[test]
    fn parses_simple_metric_above_with_empty_all_prereq() {
        let lua = Lua::new();
        let win = cond_table(&lua, |t| {
            t.set("kind", "metric_above").unwrap();
            t.set("metric", "colony_count.faction_1").unwrap();
            t.set("threshold", 1.0).unwrap();
        });
        let prereq = cond_table(&lua, |t| {
            t.set("kind", "all").unwrap();
            t.set("children", lua.create_table().unwrap()).unwrap();
        });
        let outer = lua.create_table().unwrap();
        outer.set("win", win).unwrap();
        outer.set("prerequisites", prereq).unwrap();

        let vc = parse_ai_victory_condition(&lua, outer).unwrap();
        assert_eq!(
            vc.win,
            Condition::Atom(ConditionAtom::MetricAbove {
                metric: MetricId::from("colony_count.faction_1"),
                threshold: 1.0,
            })
        );
        assert_eq!(vc.prerequisites, Condition::All(Vec::new()));
        assert_eq!(vc.time_limit, None);
        assert!(vc.score_hint.is_none());
    }

    #[test]
    fn parses_nested_all_any_not_tree() {
        let lua = Lua::new();
        // not( any( metric_below(a, 0.1), metric_above(b, 5.0) ) )
        let leaf_a = cond_table(&lua, |t| {
            t.set("kind", "metric_below").unwrap();
            t.set("metric", "a").unwrap();
            t.set("threshold", 0.1).unwrap();
        });
        let leaf_b = cond_table(&lua, |t| {
            t.set("kind", "metric_above").unwrap();
            t.set("metric", "b").unwrap();
            t.set("threshold", 5.0).unwrap();
        });
        let any_children = lua.create_table().unwrap();
        any_children.set(1, leaf_a).unwrap();
        any_children.set(2, leaf_b).unwrap();
        let any_t = cond_table(&lua, |t| {
            t.set("kind", "any").unwrap();
            t.set("children", any_children).unwrap();
        });
        let not_t = cond_table(&lua, |t| {
            t.set("kind", "not").unwrap();
            t.set("child", any_t).unwrap();
        });

        // Outer wraps a single `all` containing the `not` so we exercise
        // both the `all` combinator and the `not` combinator together.
        let all_children = lua.create_table().unwrap();
        all_children.set(1, not_t).unwrap();
        let win = cond_table(&lua, |t| {
            t.set("kind", "all").unwrap();
            t.set("children", all_children).unwrap();
        });
        let outer = lua.create_table().unwrap();
        outer.set("win", win).unwrap();

        let vc = parse_ai_victory_condition(&lua, outer).unwrap();
        let expected = Condition::All(vec![Condition::Not(Box::new(Condition::Any(vec![
            Condition::Atom(ConditionAtom::MetricBelow {
                metric: MetricId::from("a"),
                threshold: 0.1,
            }),
            Condition::Atom(ConditionAtom::MetricAbove {
                metric: MetricId::from("b"),
                threshold: 5.0,
            }),
        ])))]);
        assert_eq!(vc.win, expected);
    }

    #[test]
    fn missing_prerequisites_defaults_to_vacuous_true() {
        let lua = Lua::new();
        let win = cond_table(&lua, |t| {
            t.set("kind", "metric_above").unwrap();
            t.set("metric", "x").unwrap();
            t.set("threshold", 0.0).unwrap();
        });
        let outer = lua.create_table().unwrap();
        outer.set("win", win).unwrap();
        // No `prerequisites` field at all.

        let vc = parse_ai_victory_condition(&lua, outer).unwrap();
        // `Condition::All(vec![])` is the documented vacuous-true encoding.
        assert_eq!(vc.prerequisites, Condition::All(Vec::new()));
    }

    #[test]
    fn time_limit_round_trips() {
        let lua = Lua::new();
        let win = cond_table(&lua, |t| {
            t.set("kind", "metric_above").unwrap();
            t.set("metric", "x").unwrap();
            t.set("threshold", 0.0).unwrap();
        });
        let outer = lua.create_table().unwrap();
        outer.set("win", win).unwrap();
        outer.set("time_limit", 12345_i64).unwrap();

        let vc = parse_ai_victory_condition(&lua, outer).unwrap();
        assert_eq!(vc.time_limit, Some(12345 as Tick));
    }

    #[test]
    fn unknown_kind_returns_error() {
        let lua = Lua::new();
        let win = cond_table(&lua, |t| {
            t.set("kind", "bogus_kind").unwrap();
        });
        let outer = lua.create_table().unwrap();
        outer.set("win", win).unwrap();

        let err = parse_ai_victory_condition(&lua, outer).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("unknown `kind`") || msg.contains("bogus_kind"),
            "expected unknown-kind error, got: {msg}"
        );
    }

    #[test]
    fn always_and_never_atoms_parse() {
        let lua = Lua::new();
        let always = cond_table(&lua, |t| {
            t.set("kind", "always").unwrap();
        });
        assert_eq!(parse_ai_condition(&always).unwrap(), Condition::Always);
        let never = cond_table(&lua, |t| {
            t.set("kind", "never").unwrap();
        });
        assert_eq!(parse_ai_condition(&never).unwrap(), Condition::Never);
    }

    #[test]
    fn one_of_combinator_parses() {
        let lua = Lua::new();
        let leaf = cond_table(&lua, |t| {
            t.set("kind", "metric_present").unwrap();
            t.set("metric", "m").unwrap();
        });
        let children = lua.create_table().unwrap();
        children.set(1, leaf).unwrap();
        let one_of = cond_table(&lua, |t| {
            t.set("kind", "one_of").unwrap();
            t.set("children", children).unwrap();
        });
        let parsed = parse_ai_condition(&one_of).unwrap();
        assert_eq!(
            parsed,
            Condition::OneOf(vec![Condition::Atom(ConditionAtom::MetricPresent {
                metric: MetricId::from("m"),
            })])
        );
    }

    #[test]
    fn unsupported_atom_kind_emits_extension_hint() {
        let lua = Lua::new();
        let t = cond_table(&lua, |t| {
            t.set("kind", "compare").unwrap();
        });
        let err = parse_ai_condition(&t).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("unsupported in v1"),
            "expected unsupported-in-v1 hint, got: {msg}"
        );
    }
}
