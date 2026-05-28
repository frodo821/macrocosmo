use bevy::prelude::*;

use crate::ai::command_consumer::BuildResearchParams;
use crate::ai::command_handlers::find_empire_entity;
use crate::ai::command_params::{TECH_ID, required_str};
use crate::player::{Empire, Faction};
use crate::technology::TechId;

/// Handle `research_focus`: set the empire's active research target.
///
/// Params:
/// - `tech_id` (Str, optional): the tech to research. If absent, auto-picks
///   the first available tech whose prerequisites are met.
pub(crate) fn handle_research_focus(
    issuer: &macrocosmo_ai::FactionId,
    params: &macrocosmo_ai::CommandParams,
    empires: &Query<(Entity, &Faction), With<Empire>>,
    br: &mut BuildResearchParams,
) {
    let empire_entity = match find_empire_entity(issuer, empires) {
        Some(e) => e,
        None => {
            warn!("research_focus: no empire found for faction {:?}", issuer);
            return;
        }
    };

    let Ok((tech_tree, mut research_queue)) = br.empire_tech.get_mut(empire_entity) else {
        debug!(
            "research_focus: empire {:?} has no TechTree/ResearchQueue",
            empire_entity
        );
        return;
    };

    let tech_id = match required_str(params, TECH_ID) {
        Ok(s) => {
            let tid = TechId(s.to_string());
            if !tech_tree.can_research(&tid) {
                debug!(
                    "research_focus: tech '{}' is not researchable for empire {:?}",
                    s, empire_entity
                );
                return;
            }
            tid
        }
        _ => {
            let available = tech_tree
                .technologies
                .keys()
                .find(|tid| tech_tree.can_research(tid))
                .cloned();
            match available {
                Some(tid) => tid,
                None => {
                    debug!(
                        "research_focus: no available techs for empire {:?}",
                        empire_entity
                    );
                    return;
                }
            }
        }
    };

    research_queue.start_research(tech_id.clone());
    info!(
        "research_focus: empire {:?} now researching '{}'",
        empire_entity, tech_id.0
    );
}
