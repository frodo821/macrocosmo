//! Route classification for AI bus commands consumed by `drain_ai_commands`.

use macrocosmo_ai::CommandKindId;

use crate::ai::schema::ids::command as cmd_ids;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandRoute {
    StaleShipControl,
    Retreat,
    BuildShip,
    FortifySystem,
    ResearchFocus,
    BuildStructure,
    BuildDeliverable,
    DeployDeliverableMacro,
    Unknown,
}

pub(crate) fn classify(kind: &CommandKindId) -> CommandRoute {
    if is_stale_ship_control(kind) {
        return CommandRoute::StaleShipControl;
    }

    let kind_str = kind.as_str();
    if kind_str == cmd_ids::retreat().as_str() {
        CommandRoute::Retreat
    } else if kind_str == cmd_ids::build_ship().as_str() {
        CommandRoute::BuildShip
    } else if kind_str == cmd_ids::fortify_system().as_str() {
        CommandRoute::FortifySystem
    } else if kind_str == cmd_ids::research_focus().as_str() {
        CommandRoute::ResearchFocus
    } else if kind_str == cmd_ids::build_structure().as_str() {
        CommandRoute::BuildStructure
    } else if kind_str == cmd_ids::build_deliverable().as_str() {
        CommandRoute::BuildDeliverable
    } else if kind_str == cmd_ids::deploy_deliverable().as_str() {
        CommandRoute::DeployDeliverableMacro
    } else {
        CommandRoute::Unknown
    }
}

fn is_stale_ship_control(kind: &CommandKindId) -> bool {
    [
        cmd_ids::attack_target(),
        cmd_ids::survey_system(),
        cmd_ids::colonize_system(),
        cmd_ids::reposition(),
        cmd_ids::move_ruler(),
        cmd_ids::blockade(),
        cmd_ids::load_deliverable(),
        cmd_ids::unload_deliverable(),
        cmd_ids::colonize_planet(),
    ]
    .into_iter()
    .any(|stale_kind| *kind == stale_kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_government_commands() {
        assert_eq!(classify(&cmd_ids::retreat()), CommandRoute::Retreat);
        assert_eq!(classify(&cmd_ids::build_ship()), CommandRoute::BuildShip);
        assert_eq!(
            classify(&cmd_ids::research_focus()),
            CommandRoute::ResearchFocus
        );
        assert_eq!(
            classify(&cmd_ids::build_deliverable()),
            CommandRoute::BuildDeliverable
        );
    }

    #[test]
    fn classifies_migrated_ship_control_as_stale() {
        assert_eq!(
            classify(&cmd_ids::attack_target()),
            CommandRoute::StaleShipControl
        );
        assert_eq!(
            classify(&cmd_ids::colonize_planet()),
            CommandRoute::StaleShipControl
        );
    }

    #[test]
    fn classifies_unknown_commands() {
        assert_eq!(
            classify(&CommandKindId::from("not_a_registered_command")),
            CommandRoute::Unknown
        );
    }
}
