//! Campaign state machine transitions.

use macrocosmo_ai::campaign::{Campaign, CampaignError, CampaignState};
use macrocosmo_ai::ObjectiveId;

fn oid() -> ObjectiveId {
    ObjectiveId::from("test_objective")
}

#[test]
fn new_campaign_starts_proposed() {
    let c = Campaign::new(oid(), 10);
    assert_eq!(c.state, CampaignState::Proposed);
    assert_eq!(c.started_at, 10);
    assert_eq!(c.last_transition, 10);
}

#[test]
fn proposed_to_active_legal() {
    let mut c = Campaign::new(oid(), 0);
    assert!(c.transition(CampaignState::Active, 1).is_ok());
    assert_eq!(c.state, CampaignState::Active);
    assert_eq!(c.last_transition, 1);
}

#[test]
fn proposed_to_abandoned_legal() {
    let mut c = Campaign::new(oid(), 0);
    assert!(c.transition(CampaignState::Abandoned, 1).is_ok());
    assert_eq!(c.state, CampaignState::Abandoned);
}

#[test]
fn proposed_to_suspended_illegal() {
    let mut c = Campaign::new(oid(), 0);
    let err = c.transition(CampaignState::Suspended, 1).unwrap_err();
    assert_eq!(
        err,
        CampaignError::IllegalTransition {
            from: CampaignState::Proposed,
            to: CampaignState::Suspended
        }
    );
    assert_eq!(c.state, CampaignState::Proposed);
}

#[test]
fn active_to_all_valid_exits() {
    for to in [
        CampaignState::Suspended,
        CampaignState::Succeeded,
        CampaignState::Failed,
        CampaignState::Abandoned,
    ] {
        let mut c = Campaign::new(oid(), 0);
        c.transition(CampaignState::Active, 1).unwrap();
        assert!(c.transition(to, 2).is_ok(), "failed transition to {to:?}");
    }
}

#[test]
fn suspended_to_active_allowed() {
    let mut c = Campaign::new(oid(), 0);
    c.transition(CampaignState::Active, 1).unwrap();
    c.transition(CampaignState::Suspended, 2).unwrap();
    c.transition(CampaignState::Active, 3).unwrap();
    assert_eq!(c.state, CampaignState::Active);
}

#[test]
fn terminal_states_reject_any_transition() {
    for terminal in [
        CampaignState::Succeeded,
        CampaignState::Failed,
        CampaignState::Abandoned,
    ] {
        let mut c = Campaign::new(oid(), 0);
        c.transition(CampaignState::Active, 1).unwrap();
        c.transition(terminal, 2).unwrap();
        assert!(terminal.is_terminal());
        let err = c.transition(CampaignState::Active, 3);
        assert!(err.is_err());
    }
}

#[test]
fn failed_transition_preserves_state_and_timestamp() {
    let mut c = Campaign::new(oid(), 0);
    c.transition(CampaignState::Active, 1).unwrap();
    let _ = c.transition(CampaignState::Proposed, 99); // illegal
    assert_eq!(c.state, CampaignState::Active);
    assert_eq!(c.last_transition, 1);
}
