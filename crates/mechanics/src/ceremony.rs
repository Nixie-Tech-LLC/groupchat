//! Generic ceremony orchestration owned by mechanics.
//!
//! This module owns the ceremony-material lifecycle policy that sits between
//! the raw FROST primitives (`dkg`, `gdkg`, `reshare`, `custody`) and the
//! journaled [`crate::ledger::AuthorityLedger`]: which transcript packets are
//! **terminal** and safe to compact, and (see the state machine below) how a
//! device advances the transcripts it participates in.
//!
//! Ceremony material is a distinct journal class with its own bounded cursor;
//! only a transcript's one terminal `SpaceAuthority` effect ever enters an
//! authority frontier. Retention here is deliberately conservative: DKG
//! proposals, authorizations, DKG rounds and custody attestations are never
//! dropped by this policy (they are validation- or custody-relevant for the
//! standing authority), while completed/fenced threshold-signing transcripts
//! — whose outcome is already durable on the space plane or can never install
//! under the monotone generation rule — are reclaimed behind a durable audit
//! commitment.

use crate::dkg::{self, CeremonyOp, SignTarget};
use crate::ids::SpaceId;
use crate::space::{RootState, SignedSpaceEvent, SpaceOp};

/// The ceremony packet hashes that are **terminal** — safe to compact behind a
/// durable audit commitment — given the retained board nodes and the standing
/// space-plane state.
///
/// A packet is terminal iff it belongs to a threshold-signing transcript that
/// can no longer change anything:
///
/// - a `SignTarget::SpaceOp` transcript whose requested op names a generation
///   `<= root_state.gen`: either its terminal effect already installed (the
///   plane advanced past it) or the monotone generation rule fences it
///   forever;
/// - a `SignTarget::AuthorityGrant` transcript whose named proposal created
///   the arrangement that is now **standing** (`root_state.configuration`
///   matches): the elevation completed, and the grant's aggregated outcome
///   (`DkgAuthorize`) is retained on the board independently of the signing
///   rounds that produced it.
///
/// Everything else — active transcripts, every DKG packet, authorizations and
/// custody attestations — is retained: material an acceptor needs to validate
/// the standing authority or prove custody may not be dropped.
pub fn terminal_compactable(
    nodes: &[SignedSpaceEvent],
    space: &SpaceId,
    root_state: &RootState,
) -> Vec<String> {
    let board = dkg::parse_board(nodes, space);
    let mut drop: Vec<String> = Vec::new();
    for transcript in board.signing.values() {
        let Some(request) = transcript.request.as_ref() else {
            continue;
        };
        let CeremonyOp::SignRequest { target, op, .. } = &request.op else {
            continue;
        };
        let terminal = match target {
            SignTarget::SpaceOp => match postcard::from_bytes::<SpaceOp>(op) {
                Ok(SpaceOp::Recover { gen, .. })
                | Ok(SpaceOp::Rotate { gen, .. })
                | Ok(SpaceOp::Reshare { gen, .. }) => gen <= root_state.gen,
                // An undecodable request can never install anything; its
                // transcript is inert, but keep it (cheap, and dropping
                // unclassifiable material is the wrong default).
                Err(_) => false,
            },
            SignTarget::AuthorityGrant => match postcard::from_bytes::<dkg::AuthorityGrant>(op) {
                Ok(grant) => board
                    .dkg
                    .get(&grant.proposal)
                    .and_then(|t| t.proposal.as_ref())
                    .and_then(|p| match &p.op {
                        CeremonyOp::DkgPropose(prop) => Some(prop.configuration.id()),
                        _ => None,
                    })
                    .is_some_and(|id| id == root_state.configuration),
                Err(_) => false,
            },
        };
        if terminal {
            drop.push(request.id.to_hex());
            for round in &transcript.rounds {
                drop.push(round.id.to_hex());
            }
        }
    }
    drop.sort();
    drop.dedup();
    drop
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authority::{AuthorityConfigurationId, AuthorityId};
    use crate::ids::{DeviceId, SystemUlidSource};

    fn space() -> SpaceId {
        SpaceId::mint(&SystemUlidSource)
    }

    fn root_state(gen: u32) -> RootState {
        RootState {
            root: vec![],
            recovery_commit: [0u8; 32],
            configuration: AuthorityConfigurationId::single(),
            gen,
            recovered: gen > 0,
        }
    }

    fn seed(n: u8) -> [u8; 32] {
        [n; 32]
    }

    /// A signing request over a `Recover { gen }` plus one round, on a board
    /// with an authorized proposal so retention keeps the rounds.
    fn recover_request_nodes(ws: &SpaceId, gen: u32) -> (Vec<SignedSpaceEvent>, String) {
        // The transcript's authority: a 2-of-2 dkg proposal by device 1.
        let me = crate::crypto::device_from_seed(&seed(1));
        let other = crate::crypto::device_from_seed(&seed(2));
        let mut participants: Vec<crate::authority::PrincipalId> = vec![
            crate::authority::PrincipalId::of_device(&me),
            crate::authority::PrincipalId::of_device(&other),
        ];
        participants.sort();
        let proposal_op = CeremonyOp::DkgPropose(dkg::frost_rotation_proposal(
            [gen as u8; 16],
            2,
            participants,
            AuthorityId::single(DeviceId::from_key_string("00".repeat(32))),
        ));
        let proposal = dkg::sign_ceremony(&seed(1), &proposal_op, ws);
        let authority = dkg::TranscriptId::of(&proposal).unwrap();
        let op_bytes = postcard::to_stdvec(&SpaceOp::Recover {
            new_root: vec![crate::ids::ActorId::from_incept_hash(&"ab".repeat(32))],
            gen,
        })
        .unwrap();
        let request = dkg::sign_ceremony(
            &seed(1),
            &CeremonyOp::SignRequest {
                nonce: [gen as u8; 16],
                authority,
                target: SignTarget::SpaceOp,
                coordinator: me,
                op: op_bytes,
            },
            ws,
        );
        let signing = dkg::TranscriptId::of(&request).unwrap();
        let round = dkg::sign_ceremony(
            &seed(1),
            &CeremonyOp::SignRound1 {
                signing,
                commitments: vec![1, 2, 3],
            },
            ws,
        );
        let request_hash = request.hash();
        (vec![proposal, request, round], request_hash)
    }

    #[test]
    fn a_fenced_generation_signing_transcript_is_terminal() {
        let ws = space();
        let (nodes, request_hash) = recover_request_nodes(&ws, 1);
        // Current gen 1: the gen-1 request either installed or is fenced.
        let drop = terminal_compactable(&nodes, &ws, &root_state(1));
        assert!(drop.contains(&request_hash), "request compacts");
        // The proposal (a DKG packet) is never in the drop set.
        assert!(!drop.contains(&nodes[0].hash()), "dkg proposal retained");
    }

    #[test]
    fn an_active_next_generation_transcript_is_retained() {
        let ws = space();
        let (nodes, request_hash) = recover_request_nodes(&ws, 1);
        // Current gen 0: the gen-1 request is the ACTIVE next step.
        let drop = terminal_compactable(&nodes, &ws, &root_state(0));
        assert!(
            !drop.contains(&request_hash),
            "an active transcript may not be compacted"
        );
        assert!(drop.is_empty());
    }
}
