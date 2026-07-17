//! `lait/space/1` — the **self-certifying workspace**.
//!
//! Membership made *identity* self-certifying (`ActorId = H(inception)`); this
//! does the same for the **workspace** one layer up. A workspace id used to be a
//! random ULID, unrelated to its founder, so a joiner could not verify a
//! ticket's `founder_actor` anchor against the id — a tampered anchor forked the
//! joiner onto a genesis rooted on an attacker (see the fork class in the invite
//! review). Now the id **commits to its trust root**:
//!
//! ```text
//! workspace_id = ws_<crockford128( blake3("lait/space/1" ‖ founding_device ‖ salt) )>
//! ```
//!
//! The founding device key + a random salt are hashed into the id *before* the
//! founding actor is incepted — which breaks the circularity (an inception is
//! scoped to a workspace id, so the id cannot itself depend on the inception).
//! The founder then incepts scoped to that id, and the signed inception IS the
//! "Found" artifact: `ws_id` commits to the device, the inception commits to
//! `ws_id`, and `founder_actor = H(inception)`. A joiner given
//! `{ws_id, salt, founder_inception}` verifies the whole chain **offline**, so a
//! MITM tampering the anchor is detected rather than silently forked.

use anyhow::{bail, Result};

use crate::actor::{self, SignedEvent};
use crate::ids::{ActorId, UserId, WorkspaceId};

/// Domain separator for the workspace-id derivation.
const SPACE_DOMAIN: &[u8] = b"lait/space/1";

/// Derive the self-certifying workspace id from the founding device key + salt.
/// Pure and deterministic — every replica computes the same id, and the id is
/// bound to the founder, not random.
pub fn derive_workspace_id(founding_device: &UserId, salt: &[u8; 16]) -> WorkspaceId {
    let mut h = blake3::Hasher::new();
    h.update(SPACE_DOMAIN);
    h.update(founding_device.as_str().as_bytes());
    h.update(salt);
    let digest = h.finalize();
    let mut d16 = [0u8; 16];
    d16.copy_from_slice(&digest.as_bytes()[..16]);
    WorkspaceId::from_digest(d16)
}

/// Verify a workspace's founding commitment and return the **verified** founding
/// actor to root genesis on. Checks, all offline:
/// 1. `ws_id` commits to the inception's signing device + `salt`;
/// 2. the inception is a valid, `ws_id`-scoped founding key-event (signature,
///    workspace binding, and consent are all checked by [`actor::replay`]).
///
/// A tampered `founder_inception` (or a swapped anchor) fails (1) or (2), so a
/// joiner can never be forked onto a genesis the id does not certify.
pub fn verify_founding(
    ws_id: &WorkspaceId,
    salt: &[u8; 16],
    founder_inception: &SignedEvent,
) -> Result<ActorId> {
    // (1) The id must commit to the device that signed the founding inception.
    if derive_workspace_id(&founder_inception.author, salt) != *ws_id {
        bail!("workspace id does not commit to this founder — ticket is forged or corrupt");
    }
    // (2) The inception must validly incept for THIS workspace. `actor::replay`
    // verifies the envelope signature, the workspace binding, and the device
    // consent, and only then does the actor `exists`.
    let founder_actor = ActorId::from_incept_hash(&founder_inception.hash());
    let plane = actor::replay(ws_id, std::slice::from_ref(founder_inception));
    if !plane.exists(&founder_actor) {
        bail!("founding inception is not valid for this workspace");
    }
    Ok(founder_actor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::incept_single;

    fn founding(seed: [u8; 32], salt: [u8; 16]) -> (WorkspaceId, SignedEvent, ActorId) {
        let device = {
            let sk = ed25519_dalek::SigningKey::from_bytes(&seed);
            UserId::from_key_string(data_encoding::HEXLOWER.encode(sk.verifying_key().as_bytes()))
        };
        let ws = derive_workspace_id(&device, &salt);
        let (incept, actor) = incept_single(&seed, &ws, [1u8; 16], [2u8; 16], None);
        (ws, incept, actor)
    }

    #[test]
    fn a_valid_founding_verifies_to_its_actor() {
        let salt = [9u8; 16];
        let (ws, incept, actor) = founding([7u8; 32], salt);
        assert_eq!(
            verify_founding(&ws, &salt, &incept).unwrap(),
            actor,
            "a genuine founding verifies to its founding actor"
        );
        // The id is a well-formed, parseable workspace id.
        assert!(WorkspaceId::parse(ws.as_str()).is_some());
    }

    #[test]
    fn a_swapped_founder_inception_is_rejected() {
        let salt = [9u8; 16];
        let (ws, _real_incept, _real_actor) = founding([7u8; 32], salt);
        // An attacker's inception (a different device) for the same ws + salt:
        // the id no longer commits to it, so verification fails.
        let (_ws2, evil_incept, _evil_actor) = founding([8u8; 32], salt);
        assert!(
            verify_founding(&ws, &salt, &evil_incept).is_err(),
            "an inception the id does not commit to is rejected"
        );
    }

    #[test]
    fn a_tampered_salt_is_rejected() {
        let (ws, incept, _actor) = founding([7u8; 32], [9u8; 16]);
        assert!(
            verify_founding(&ws, &[0u8; 16], &incept).is_err(),
            "a salt that does not reproduce the id is rejected"
        );
    }
}
