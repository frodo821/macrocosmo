use bevy::prelude::*;

/// Authoritative game simulation without UI, rendering, input, or remote control.
pub struct SimulationPlugin;

impl Plugin for SimulationPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            crate::time_system::GameTimePlugin,
            crate::galaxy::GalaxyPlugin,
            crate::player::PlayerPlugin,
            crate::communication::CommunicationPlugin,
            crate::knowledge::KnowledgePlugin,
            crate::ship::ShipPlugin,
            crate::colony::ColonyPlugin,
            crate::scripting::ScriptingPlugin,
            crate::technology::TechnologyPlugin,
            crate::event_system::EventSystemPlugin,
            crate::events::EventsPlugin,
            crate::species::SpeciesPlugin,
            crate::ship_design::ShipDesignPlugin,
        ))
        .add_plugins((
            crate::deep_space::DeepSpacePlugin,
            crate::setup::GameSetupPlugin,
            crate::notifications::NotificationsPlugin,
            crate::faction::FactionRelationsPlugin,
            crate::choice::ChoicesPlugin,
            crate::ai::AiPlugin,
            crate::casus_belli::CasusBelliPlugin,
            crate::observer::ObserverSimulationPlugin,
        ));
    }
}
