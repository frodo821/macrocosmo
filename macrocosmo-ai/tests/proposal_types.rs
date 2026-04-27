//! Confirm Proposal / ProposalOutcome / ConflictKind / Locality
//! are part of macrocosmo-ai's public surface so downstream
//! crates (macrocosmo) can use them directly.

use macrocosmo_ai::{ConflictKind, Locality, Proposal, ProposalOutcome};

#[test]
fn public_re_exports_compile() {
    let _l = Locality::FactionWide;
    let _ck = ConflictKind::OutOfRegion;
    let _po = ProposalOutcome::Accepted;
    // Proposal needs Command — just verify the type is reachable.
    fn _accept_proposal(_p: Proposal) {}
}
