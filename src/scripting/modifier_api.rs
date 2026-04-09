use crate::amount::SignedAmt;
use crate::modifier::{ModifiedValue, Modifier};

/// Parse a Lua table into a Modifier.
pub fn parse_modifier_table(
    table: &mlua::Table,
    clock_elapsed: i64,
) -> Result<Modifier, mlua::Error> {
    let id: String = table.get("id")?;
    let label: String = table.get::<Option<String>>("label")?.unwrap_or_default();
    let base_add = table
        .get::<Option<f64>>("base_add")?
        .map(SignedAmt::from_f64)
        .unwrap_or(SignedAmt::ZERO);
    let multiplier = table
        .get::<Option<f64>>("multiplier")?
        .map(SignedAmt::from_f64)
        .unwrap_or(SignedAmt::ZERO);
    let add = table
        .get::<Option<f64>>("add")?
        .map(SignedAmt::from_f64)
        .unwrap_or(SignedAmt::ZERO);
    let duration: Option<i64> = table.get("duration")?;
    let expires_at = duration.map(|d| clock_elapsed + d);
    let on_expire_event: Option<String> = table.get("on_expire_event")?;

    Ok(Modifier {
        id,
        label,
        base_add,
        multiplier,
        add,
        expires_at,
        on_expire_event,
    })
}

/// Convert a ModifiedValue's info to a Lua table for reading.
pub fn modified_value_to_lua(
    lua: &mlua::Lua,
    mv: &ModifiedValue,
    current_time: i64,
) -> Result<mlua::Table, mlua::Error> {
    let table = lua.create_table()?;
    table.set("base", mv.base().to_f64())?;
    table.set("effective_base", mv.effective_base().to_f64())?;
    table.set("multiplier", mv.total_multiplier().raw() as f64 / 1000.0)?;
    table.set("add", mv.total_add().raw() as f64 / 1000.0)?;
    table.set("final", mv.final_value().to_f64())?;

    let modifiers = lua.create_table()?;
    for (i, m) in mv.modifiers().iter().enumerate() {
        let mt = lua.create_table()?;
        mt.set("id", m.id.as_str())?;
        mt.set("label", m.label.as_str())?;
        mt.set("base_add", m.base_add.raw() as f64 / 1000.0)?;
        mt.set("multiplier", m.multiplier.raw() as f64 / 1000.0)?;
        mt.set("add", m.add.raw() as f64 / 1000.0)?;
        if let Some(remaining) = m.remaining_duration(current_time) {
            mt.set("remaining_duration", remaining)?;
        }
        if let Some(ref evt) = m.on_expire_event {
            mt.set("on_expire_event", evt.as_str())?;
        }
        modifiers.set(i + 1, mt)?;
    }
    table.set("modifiers", modifiers)?;

    Ok(table)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::amount::Amt;

    #[test]
    fn test_parse_modifier_table() {
        let lua = mlua::Lua::new();
        let table = lua.create_table().unwrap();
        table.set("id", "test_mod").unwrap();
        table.set("label", "Test Modifier").unwrap();
        table.set("base_add", 3.0).unwrap();
        table.set("multiplier", 0.15).unwrap();
        table.set("add", -1.5).unwrap();
        table.set("duration", 10i64).unwrap();
        table.set("on_expire_event", "boom").unwrap();

        let m = parse_modifier_table(&table, 5).unwrap();
        assert_eq!(m.id, "test_mod");
        assert_eq!(m.label, "Test Modifier");
        assert_eq!(m.base_add, SignedAmt::from_f64(3.0));
        assert_eq!(m.multiplier, SignedAmt::from_f64(0.15));
        assert_eq!(m.add, SignedAmt::from_f64(-1.5));
        assert_eq!(m.expires_at, Some(15)); // clock_elapsed=5 + duration=10
        assert_eq!(m.on_expire_event, Some("boom".to_string()));
    }

    #[test]
    fn test_parse_modifier_table_minimal() {
        let lua = mlua::Lua::new();
        let table = lua.create_table().unwrap();
        table.set("id", "minimal").unwrap();

        let m = parse_modifier_table(&table, 0).unwrap();
        assert_eq!(m.id, "minimal");
        assert_eq!(m.label, "");
        assert_eq!(m.base_add, SignedAmt::ZERO);
        assert_eq!(m.multiplier, SignedAmt::ZERO);
        assert_eq!(m.add, SignedAmt::ZERO);
        assert_eq!(m.expires_at, None);
        assert_eq!(m.on_expire_event, None);
    }

    #[test]
    fn test_modified_value_to_lua() {
        let lua = mlua::Lua::new();

        let mut mv = ModifiedValue::new(Amt::units(10));
        mv.push_modifier(Modifier {
            id: "buff".to_string(),
            label: "Buff".to_string(),
            base_add: SignedAmt::units(2),
            multiplier: SignedAmt::new(0, 100), // +10%
            add: SignedAmt::units(1),
            expires_at: Some(20),
            on_expire_event: Some("buff_gone".to_string()),
        });

        let table = modified_value_to_lua(&lua, &mv, 5).unwrap();

        let base: f64 = table.get("base").unwrap();
        assert!((base - 10.0).abs() < 1e-10);

        let effective_base: f64 = table.get("effective_base").unwrap();
        assert!((effective_base - 12.0).abs() < 1e-10);

        let multiplier: f64 = table.get("multiplier").unwrap();
        assert!((multiplier - 1.1).abs() < 1e-10);

        let add: f64 = table.get("add").unwrap();
        assert!((add - 1.0).abs() < 1e-10);

        let final_val: f64 = table.get("final").unwrap();
        // (10+2)*1.1 + 1 = 13.2 + 1 = 14.2
        assert!((final_val - 14.2).abs() < 1e-10);

        let modifiers: mlua::Table = table.get("modifiers").unwrap();
        assert_eq!(modifiers.len().unwrap(), 1);

        let mt: mlua::Table = modifiers.get(1).unwrap();
        let id: String = mt.get("id").unwrap();
        assert_eq!(id, "buff");

        let remaining: i64 = mt.get("remaining_duration").unwrap();
        assert_eq!(remaining, 15); // expires_at=20, current=5

        let evt: String = mt.get("on_expire_event").unwrap();
        assert_eq!(evt, "buff_gone");
    }
}
