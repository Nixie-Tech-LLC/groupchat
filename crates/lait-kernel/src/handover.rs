//! D3 — rotation and disjoint handover.
//!
//! A rotation installs a **new, independent** authority: a fresh [`crate::gdkg`]
//! run produces a new key `Y₂` unrelated to the old `Y₁`. Because the new key is
//! independent, the old and new holder sets need not overlap — disjoint handover
//! is exactly as valid as an overlapping one. What makes the handover safe is not
//! shared shares but a signature: **the old authority signs the installation**,
//! and only after it has pinned the exact candidate key, configuration,
//! transcript evidence and activation custody rule. No old holder trusts a key
//! merely because a new participant claims to have derived it.
//!
//! This module defines the bytes the old authority signs ([`InstallationTerms`]),
//! and — given a set of *signed* installations — decides which one wins and
//! projects the outcome ([`resolve`]). Selection verifies each signature under
//! `Y₁` and picks the smallest transition id among the authorized ones, so every
//! replica converges without coordination. The signature itself is an ordinary
//! [`crate::gaccess`] signature under `Y₁`, so a solo old key (1-of-1), a flat
//! FROST old key (k-of-n) and a general-policy old key all install a successor
//! the same way.
//!
//! # ⚠ Review boundary — UNREVIEWED functional prototype
//!
//! The [`crate::gaccess`]/[`crate::gdkg`] boundaries carry over. **Scope:** this
//! module binds and authorizes the *installation signature* and decides the race
//! among authorized installations. It deliberately does **not** validate a
//! candidate's possession evidence, its signing plan/witness, the custody acks,
//! or transition readiness — those are the C4/D6 acceptance checks that must pass
//! before an installation is signed, and they live above this module. The
//! partition-tolerant agreement and liveness layer around all of this is the
//! reviewed deliverable. Wired into nothing.

use crate::authority::{AuthorityConfigurationId, LeafId};
use crate::gaccess::{self, KeyShares, Signature};
use crate::ids::UserId;
use crate::transition::{CandidateAuthority, TransitionId, TransitionState};

const INSTALL_DOMAIN: &[u8] = b"lait/space/1/handover/1/install";

/// Exactly what the old authority signs off on to install a successor: the
/// transition, the new configuration, the new public key, the transcript
/// commitment the candidate came from, and the activation custody rule (which
/// leaves must have attested a usable share before activation).
///
/// These are precisely the [`CandidateAuthority`] fields an old holder must
/// verify (§27); [`InstallationTerms::for_candidate`] projects a C4 candidate
/// record onto them, and `required_leaves` is the C4 activation rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallationTerms {
    transition: TransitionId,
    configuration: AuthorityConfigurationId,
    new_public_key: UserId,
    transcript_commitment: [u8; 32],
    /// The activation custody rule: leaves whose share must be attested. Kept
    /// sorted and deduped so the message is canonical.
    required_leaves: Vec<LeafId>,
}

impl InstallationTerms {
    /// Build terms directly (tests, and callers that already hold the parts).
    /// `required_leaves` is sorted and deduped for a canonical message.
    pub fn new(
        transition: TransitionId,
        configuration: AuthorityConfigurationId,
        new_public_key: UserId,
        transcript_commitment: [u8; 32],
        required_leaves: Vec<LeafId>,
    ) -> Self {
        let mut required_leaves = required_leaves;
        required_leaves.sort();
        required_leaves.dedup();
        Self {
            transition,
            configuration,
            new_public_key,
            transcript_commitment,
            required_leaves,
        }
    }

    /// Project a completed C4 candidate record onto the terms the old authority
    /// signs, paired with the activation custody rule for the transition.
    pub fn for_candidate(candidate: &CandidateAuthority, required_leaves: Vec<LeafId>) -> Self {
        Self::new(
            candidate.transition,
            candidate.configuration,
            candidate.public_key.clone(),
            candidate.transcript_commitment,
            required_leaves,
        )
    }

    /// The new public key these terms install.
    pub fn new_public_key(&self) -> &UserId {
        &self.new_public_key
    }

    /// The transition these terms install.
    pub fn transition(&self) -> TransitionId {
        self.transition
    }

    /// The canonical, domain-separated message the old authority signs. Every
    /// field is length-prefixed so no two distinct term sets share an encoding.
    pub fn message(&self) -> Vec<u8> {
        let mut m = Vec::new();
        m.extend_from_slice(INSTALL_DOMAIN);
        push_bytes(&mut m, self.transition.to_hex().as_bytes());
        push_bytes(&mut m, self.configuration.to_hex().as_bytes());
        push_bytes(&mut m, self.new_public_key.as_str().as_bytes());
        push_bytes(&mut m, &self.transcript_commitment);
        m.extend_from_slice(&(self.required_leaves.len() as u64).to_le_bytes());
        for leaf in &self.required_leaves {
            push_bytes(&mut m, leaf.as_str().as_bytes());
        }
        m
    }
}

fn push_bytes(buf: &mut Vec<u8>, bytes: &[u8]) {
    buf.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    buf.extend_from_slice(bytes);
}

/// Verify that the old authority (public key `old_public_key`) signed exactly
/// these installation terms. This is the check every old holder — and every
/// replica applying the rotation — runs before accepting the successor.
pub fn verify_installation(
    old_public_key: &[u8; 32],
    terms: &InstallationTerms,
    signature: &Signature,
) -> bool {
    gaccess::verify(old_public_key, &terms.message(), signature)
}

/// Sign installation terms with a qualified set of the **old** authority. A thin
/// pass-through to [`gaccess::sign_qualified`] over [`InstallationTerms::message`],
/// so the old key installs a successor with the same signer machinery it uses for
/// anything else.
#[allow(clippy::too_many_arguments)]
pub fn sign_installation<K: KeyShares>(
    old_key: &K,
    witness: &crate::compile::ReconstructionWitness,
    nonces: &std::collections::BTreeMap<LeafId, gaccess::Nonce>,
    commitments: &[(LeafId, gaccess::Commitment)],
    terms: &InstallationTerms,
) -> Option<Signature> {
    gaccess::sign_qualified(witness, old_key, nonces, commitments, &terms.message())
}

/// An installation the old authority signed: the terms plus the signature over
/// them. The unit [`resolve`] selects among when candidates race.
#[derive(Debug, Clone)]
pub struct SignedInstallation {
    pub terms: InstallationTerms,
    pub signature: Signature,
}

/// Project a *already-decided* race: given the competing transitions and the one
/// that won, mark it [`TransitionState::Activated`] and every other
/// [`TransitionState::Superseded`]. Deterministic, sorted by transition id.
/// Returns `None` if `installed` is not among `candidates`.
///
/// This is pure projection — it does **not** decide the winner. [`resolve`] does
/// that from signed installations; call this only when the winner is already
/// established (e.g. re-deriving state from a recorded outcome).
pub fn project_installed(
    candidates: &[TransitionId],
    installed: TransitionId,
) -> Option<Vec<(TransitionId, TransitionState)>> {
    if !candidates.contains(&installed) {
        return None;
    }
    let mut out: Vec<(TransitionId, TransitionState)> = candidates
        .iter()
        .map(|&t| {
            let state = if t == installed {
                TransitionState::Activated
            } else {
                TransitionState::Superseded
            };
            (t, state)
        })
        .collect();
    out.sort_by_key(|(t, _)| t.to_hex());
    out.dedup_by(|a, b| a.0 == b.0);
    Some(out)
}

/// Decide a race among concurrent signed installations and project the outcome.
///
/// Each installation is verified under `old_public_key`; ones whose signature
/// does not check are excluded entirely (an unauthorized installation cannot
/// win). Among the *valid* ones, the winner is selected deterministically — the
/// smallest transition id, which is content-addressed, so every replica agrees
/// without coordination. The result activates the winner and supersedes the
/// other valid candidates.
///
/// Returns `None` if no installation is valid — there is then no authorized
/// successor, and the arrangement stays put rather than entering an ambiguous
/// state. Invalid installations do not appear in the projection at all.
///
/// Scope: this decides *which authorized installation wins*. It does **not**
/// validate candidate possession evidence, the custody acks, or transition
/// readiness — those are the C4/D6 acceptance checks that must pass *before* an
/// installation is signed, and they live above this module.
pub fn resolve(
    old_public_key: &[u8; 32],
    installations: &[SignedInstallation],
) -> Option<Vec<(TransitionId, TransitionState)>> {
    // Keep only authorized installations, deduped by transition.
    let mut valid: Vec<TransitionId> = Vec::new();
    for si in installations {
        if verify_installation(old_public_key, &si.terms, &si.signature) {
            let t = si.terms.transition();
            if !valid.contains(&t) {
                valid.push(t);
            }
        }
    }
    let winner = *valid.iter().min_by_key(|t| t.to_hex())?;
    project_installed(&valid, winner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authority::PrincipalId;
    use crate::compile::{compile, StructurallyValidatedCompiledPolicy};
    use crate::expand::{expand, PrincipalCustody, PrincipalDescriptor};
    use crate::gaccess::{commit, Nonce};
    use crate::gdkg::{aggregate, contribute, GroupKey};
    use crate::policy::OwnershipPolicy;
    use std::collections::BTreeMap;

    fn prin(n: u8) -> PrincipalId {
        PrincipalId::of_device(&crate::crypto::user_from_seed(&[n; 32]))
    }
    fn key(n: u8) -> OwnershipPolicy {
        OwnershipPolicy::Key(prin(n))
    }
    fn resolver() -> impl Fn(&PrincipalId) -> Option<PrincipalDescriptor> {
        |p: &PrincipalId| {
            Some(PrincipalDescriptor {
                id: p.clone(),
                custody: PrincipalCustody::Direct {
                    device: p.as_device()?,
                },
            })
        }
    }
    fn compiled(o: OwnershipPolicy) -> (StructurallyValidatedCompiledPolicy, Vec<LeafId>) {
        let canon = o.canonicalize().unwrap();
        let exp = expand(&canon, &resolver()).unwrap();
        let c = compile(&exp).unwrap();
        let leaves = c.leaves().to_vec();
        (c, leaves)
    }
    /// Run a dealer-free DKG over the compiled policy (every leaf contributes).
    fn dkg(c: &StructurallyValidatedCompiledPolicy, leaves: &[LeafId]) -> GroupKey {
        let contribs: Vec<_> = leaves.iter().map(|l| contribute(c, l.clone())).collect();
        aggregate(c, &contribs).expect("aggregate")
    }
    /// A UserId identity for a DKG public key.
    fn user_of(group: &GroupKey) -> UserId {
        UserId::from_key_string(data_encoding::HEXLOWER.encode(&group.public_key()))
    }
    fn tid(byte: u8) -> TransitionId {
        TransitionId::parse_hex(&data_encoding::HEXLOWER.encode(&[byte; 32])).unwrap()
    }
    /// Have a qualified set of `old` sign the given installation terms.
    fn old_signs(
        old_c: &StructurallyValidatedCompiledPolicy,
        old_key: &GroupKey,
        signers: &[LeafId],
        terms: &InstallationTerms,
    ) -> Signature {
        let witness = old_c.reconstruct(signers).expect("old set is qualified");
        let mut nonces: BTreeMap<LeafId, Nonce> = BTreeMap::new();
        let mut commitments = Vec::new();
        for leaf in &witness.leaves {
            let (n, com) = commit();
            nonces.insert(leaf.clone(), n);
            commitments.push((leaf.clone(), com));
        }
        sign_installation(old_key, &witness, &nonces, &commitments, terms).expect("sign install")
    }

    /// The core D3 flow: an `old` authority installs a fresh `new` authority.
    /// Returns whether the installation verifies under the old key.
    fn handover(
        old_policy: OwnershipPolicy,
        old_signers: impl Fn(&[LeafId]) -> Vec<LeafId>,
        new_policy: OwnershipPolicy,
    ) -> bool {
        let (old_c, old_leaves) = compiled(old_policy);
        let old_key = dkg(&old_c, &old_leaves);

        let (new_c, new_leaves) = compiled(new_policy);
        let new_key = dkg(&new_c, &new_leaves);

        // Terms bind the *new* key + config + a transcript commitment + the
        // activation rule (here: every new leaf must be backed).
        let terms = InstallationTerms::new(
            tid(0xC1),
            AuthorityConfigurationId::single(), // stand-in id; identity is what's bound
            user_of(&new_key),
            [7u8; 32],
            new_leaves.clone(),
        );
        let sig = old_signs(&old_c, &old_key, &old_signers(&old_leaves), &terms);
        verify_installation(&old_key.public_key(), &terms, &sig)
    }

    #[test]
    fn solo_to_policy() {
        // Old: a single key. New: 2-of-3.
        assert!(handover(
            key(1),
            |leaves| vec![leaves[0].clone()],
            OwnershipPolicy::Threshold {
                k: 2,
                members: vec![key(2), key(3), key(4)],
            },
        ));
    }

    #[test]
    fn flat_frost_to_policy() {
        // Old: flat 2-of-3 threshold. New: compartmented policy.
        assert!(handover(
            OwnershipPolicy::Threshold {
                k: 2,
                members: vec![key(1), key(2), key(3)],
            },
            |leaves| vec![leaves[0].clone(), leaves[1].clone()],
            OwnershipPolicy::AllOf(vec![OwnershipPolicy::AnyOf(vec![key(4), key(5)]), key(6)]),
        ));
    }

    #[test]
    fn policy_to_overlapping_policy() {
        // Old and new share holders {1,2}; the key is still independent.
        assert!(handover(
            OwnershipPolicy::Threshold {
                k: 2,
                members: vec![key(1), key(2), key(3)],
            },
            |leaves| vec![leaves[0].clone(), leaves[1].clone()],
            OwnershipPolicy::Threshold {
                k: 2,
                members: vec![key(1), key(2), key(4)],
            },
        ));
    }

    #[test]
    fn policy_to_wholly_disjoint_policy() {
        // No shared holders at all — the whole point of D3.
        let old_prins = [prin(1), prin(2), prin(3)];
        let new_prins = [prin(4), prin(5), prin(6)];
        assert!(old_prins.iter().all(|p| !new_prins.contains(p)));
        assert!(handover(
            OwnershipPolicy::Threshold {
                k: 2,
                members: vec![key(1), key(2), key(3)],
            },
            |leaves| vec![leaves[0].clone(), leaves[2].clone()],
            OwnershipPolicy::Threshold {
                k: 2,
                members: vec![key(4), key(5), key(6)],
            },
        ));
    }

    #[test]
    fn an_installation_is_bound_to_the_exact_new_key() {
        // A signature for candidate A's terms must not verify for candidate B.
        let (old_c, old_leaves) = compiled(OwnershipPolicy::Threshold {
            k: 2,
            members: vec![key(1), key(2), key(3)],
        });
        let old_key = dkg(&old_c, &old_leaves);

        let (a_c, a_leaves) = compiled(OwnershipPolicy::Key(key_prin(4)));
        let a_key = dkg(&a_c, &a_leaves);
        let (b_c, b_leaves) = compiled(OwnershipPolicy::Key(key_prin(5)));
        let b_key = dkg(&b_c, &b_leaves);

        let terms_a = InstallationTerms::new(
            tid(0xAA),
            AuthorityConfigurationId::single(),
            user_of(&a_key),
            [1u8; 32],
            a_leaves.clone(),
        );
        let sig = old_signs(
            &old_c,
            &old_key,
            &[old_leaves[0].clone(), old_leaves[1].clone()],
            &terms_a,
        );
        assert!(verify_installation(&old_key.public_key(), &terms_a, &sig));

        // Same signature, terms naming B's key instead: must fail.
        let terms_b = InstallationTerms::new(
            tid(0xAA),
            AuthorityConfigurationId::single(),
            user_of(&b_key),
            [1u8; 32],
            b_leaves.clone(),
        );
        assert!(!verify_installation(&old_key.public_key(), &terms_b, &sig));

        // And a tampered signature fails.
        let mut bad = sig;
        bad.z[0] ^= 1;
        assert!(!verify_installation(&old_key.public_key(), &terms_a, &bad));
    }

    // A one-leaf policy from a distinct principal, for the binding test above.
    fn key_prin(n: u8) -> PrincipalId {
        prin(n)
    }

    /// A signed installation for a fresh candidate key (seeded by `cand_seed`)
    /// under transition `transition`.
    fn signed_installation(
        old_c: &StructurallyValidatedCompiledPolicy,
        old_key: &GroupKey,
        old_signers: &[LeafId],
        transition: TransitionId,
        cand_seed: u8,
    ) -> SignedInstallation {
        let (cand_c, cand_leaves) = compiled(OwnershipPolicy::Key(prin(cand_seed)));
        let cand_key = dkg(&cand_c, &cand_leaves);
        let terms = InstallationTerms::new(
            transition,
            AuthorityConfigurationId::single(),
            user_of(&cand_key),
            [cand_seed; 32],
            cand_leaves,
        );
        let signature = old_signs(old_c, old_key, old_signers, &terms);
        SignedInstallation { terms, signature }
    }

    #[test]
    fn resolve_selects_the_deterministic_winner_among_signed_installations() {
        let (old_c, old_leaves) = compiled(OwnershipPolicy::Threshold {
            k: 2,
            members: vec![key(1), key(2), key(3)],
        });
        let old_key = dkg(&old_c, &old_leaves);
        let signers = vec![old_leaves[0].clone(), old_leaves[1].clone()];

        // Two authorized, racing installations with distinct transition ids.
        let a = tid(0x01);
        let b = tid(0x02);
        let inst_a = signed_installation(&old_c, &old_key, &signers, a, 40);
        let inst_b = signed_installation(&old_c, &old_key, &signers, b, 41);

        let resolved = resolve(&old_key.public_key(), &[inst_b.clone(), inst_a.clone()])
            .expect("an authorized installation exists");
        // Smallest transition id (a) wins regardless of input order.
        let activated: Vec<_> = resolved
            .iter()
            .filter(|(_, s)| *s == TransitionState::Activated)
            .map(|(t, _)| *t)
            .collect();
        assert_eq!(activated, vec![a], "min transition id wins");
        assert_eq!(
            resolved
                .iter()
                .filter(|(_, s)| *s == TransitionState::Superseded)
                .count(),
            1
        );
    }

    #[test]
    fn resolve_excludes_unauthorized_installations() {
        let (old_c, old_leaves) = compiled(OwnershipPolicy::Threshold {
            k: 2,
            members: vec![key(1), key(2), key(3)],
        });
        let old_key = dkg(&old_c, &old_leaves);
        let signers = vec![old_leaves[0].clone(), old_leaves[1].clone()];

        // A valid installation for b, and a *forged* one for a (tampered signature).
        let a = tid(0x01);
        let b = tid(0x02);
        let mut forged = signed_installation(&old_c, &old_key, &signers, a, 40);
        forged.signature.z[0] ^= 1; // break the signature
        let inst_b = signed_installation(&old_c, &old_key, &signers, b, 41);

        let resolved = resolve(&old_key.public_key(), &[forged, inst_b]).expect("b is authorized");
        // The forged a does not win despite its smaller id, and does not appear.
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0], (b, TransitionState::Activated));

        // With no valid installation, there is no successor — not an ambiguous one.
        let mut all_forged = signed_installation(&old_c, &old_key, &signers, a, 40);
        all_forged.signature.z[0] ^= 1;
        assert_eq!(resolve(&old_key.public_key(), &[all_forged]), None);
    }

    #[test]
    fn project_installed_is_a_pure_deterministic_projection() {
        let a = tid(0x01);
        let b = tid(0x02);
        let c = tid(0x03);
        let resolved = project_installed(&[a, b, c], b).expect("installed is in the race");
        let activated: Vec<_> = resolved
            .iter()
            .filter(|(_, s)| *s == TransitionState::Activated)
            .map(|(t, _)| *t)
            .collect();
        assert_eq!(activated, vec![b]);
        // Deterministic regardless of input order.
        assert_eq!(resolved, project_installed(&[c, b, a], b).unwrap());
        // Projecting a transition that never raced is refused.
        assert_eq!(project_installed(&[a, c], b), None);
    }
}
