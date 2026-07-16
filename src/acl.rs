//! Membership / ACL — a signed ed25519 op-graph validated by deterministic
//! replay (S§6, A§11), riding the shared hash-DAG envelope ([`crate::sigdag`],
//! domain `lait/aclop/1`). The ops ride in the plaintext membership layer
//! ([`crate::membership`]) and propagate as a grow-only set; **trust is computed
//! by app-layer replay**, never by Loro. Revocation is **remove-wins**.
//!
//! Principals (LAIT-DATA-CONTRACT §3.4):
//! - `admin` — may author every membership op.
//! - `member` — a human collaborator; may sponsor agents and remove their own.
//! - **agent** — an AI keypair sponsored by a member (`AddAgent`, sponsor = the
//!   op's author). An agent is a member for *encryption* purposes (it is sealed
//!   the workspace key) but may author **no** membership op, and its membership
//!   dies with its sponsor — removing the human transitively ends every agent
//!   they sponsored. No re-delegation: agents cannot sponsor agents.
//!
//! Authority: an op is honored only if its author holds the required standing
//! as of the op's causal history. Sequential actions (the common path) validate
//! exactly; concurrency resolves by deterministic topo tie-break, with the
//! remove-wins override over the causal ancestor closure so a concurrent add
//! cannot override a remove.
//!
//! Forward compatibility: a signature-valid op whose bytes this build cannot
//! decode is kept as an **opaque DAG node** — present for ancestry, no effect
//! on state — so future op kinds cannot diverge two replicas' ancestor
//! closures (which would diverge membership, and therefore key-sealing).
//!
//! > **Research-grade** (A§2): a proven design implemented by hand, unaudited.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

use crate::ids::{UserId, WorkspaceId};
use crate::sigdag::{self, SignedNode};
use crate::store::Genesis;

/// The signing domain for membership ops (see [`crate::sigdag`]).
pub const ACL_DOMAIN: &[u8] = b"lait/aclop/1";

/// A signed membership op — the shared envelope under this plane's domain.
pub type SignedOp = SignedNode;

/// A member role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    Member,
}

/// A membership operation (S§6). Canonically encoded (postcard) and signed.
/// Variants are **append-only** (postcard discriminants are positional).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AclOp {
    AddMember {
        key: UserId,
        role: Role,
    },
    RemoveMember {
        key: UserId,
    },
    SetRole {
        key: UserId,
        role: Role,
    },
    /// Sponsor an agent keypair (contract §3.4). The sponsor is the op's
    /// **author**; the agent's membership is derived, and dies, with them.
    AddAgent {
        key: UserId,
    },
}

impl AclOp {
    fn key(&self) -> &UserId {
        match self {
            AclOp::AddMember { key, .. }
            | AclOp::RemoveMember { key }
            | AclOp::SetRole { key, .. }
            | AclOp::AddAgent { key } => key,
        }
    }
    fn encode(&self) -> Vec<u8> {
        postcard::to_stdvec(self).expect("encode acl op")
    }
}

/// Sign an [`AclOp`] with the author's ed25519 seed, given the current heads as
/// parents and the workspace id (S§6). The signature binds op ‖ author ‖
/// sorted(parents) ‖ workspace id under the plane domain, so a valid op cannot
/// be re-parented, replayed across workspaces, or lifted into another plane.
pub fn sign_op(
    seed: &[u8; 32],
    op: &AclOp,
    parents: Vec<String>,
    workspace_id: &WorkspaceId,
) -> SignedOp {
    sigdag::sign_node(
        ACL_DOMAIN,
        seed,
        op.encode(),
        parents,
        workspace_id.as_str(),
    )
}

/// The materialized ACL state after replay.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AclState {
    /// Every principal sealed into the workspace, humans and agents alike.
    members: BTreeMap<UserId, Role>,
    /// agent key → sponsoring member. Every key here is also in `members`
    /// (with `Role::Member`); an agent's presence is derived from its sponsor's.
    agents: BTreeMap<UserId, UserId>,
}

impl AclState {
    /// Whether `u` is sealed into the workspace (humans and agents alike).
    pub fn is_member(&self, u: &UserId) -> bool {
        self.members.contains_key(u)
    }
    pub fn is_admin(&self, u: &UserId) -> bool {
        matches!(self.members.get(u), Some(Role::Admin))
    }
    /// Whether `u` is an agent principal (contract §3.4).
    pub fn is_agent(&self, u: &UserId) -> bool {
        self.agents.contains_key(u)
    }
    /// The sponsoring member of an agent.
    pub fn sponsor_of(&self, u: &UserId) -> Option<&UserId> {
        self.agents.get(u)
    }
    /// A human (non-agent) member — the standing content-authority ops require.
    pub fn is_human_member(&self, u: &UserId) -> bool {
        self.is_member(u) && !self.is_agent(u)
    }
    pub fn role(&self, u: &UserId) -> Option<Role> {
        self.members.get(u).copied()
    }
    /// `admin` | `member` | `agent` — the projection surface.
    pub fn standing(&self, u: &UserId) -> Option<&'static str> {
        if self.is_agent(u) {
            return Some("agent");
        }
        match self.members.get(u)? {
            Role::Admin => Some("admin"),
            Role::Member => Some("member"),
        }
    }
    /// All current members, sorted by key (includes agents — the sealing set).
    pub fn members(&self) -> Vec<(UserId, Role)> {
        self.members.iter().map(|(k, v)| (k.clone(), *v)).collect()
    }
    /// All current agents with their sponsors, sorted by key.
    pub fn agents(&self) -> Vec<(UserId, UserId)> {
        self.agents
            .iter()
            .map(|(k, s)| (k.clone(), s.clone()))
            .collect()
    }
    pub fn len(&self) -> usize {
        self.members.len()
    }
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }
}

/// One rendered row of the membership audit log (`lait members log`): the op
/// in deterministic causal order, with its replay verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEntry {
    pub hash: String,
    pub author: UserId,
    /// `add_member` | `remove_member` | `set_role` | `add_agent` | `unknown`.
    pub kind: &'static str,
    /// The subject key (absent for undecodable ops).
    pub subject: Option<UserId>,
    pub role: Option<Role>,
    /// Whether replay honored the op (false = unauthorized or undecodable).
    pub authorized: bool,
}

/// Deterministically replay a signed op-graph from the genesis (S§6). Founding
/// admins seed the admin set; each op is honored only if signature-valid and
/// authored by a principal with the required standing as of its causal history;
/// membership resolves **remove-wins** over the causal ancestor closure, then
/// agents cascade with their sponsors.
pub fn replay(genesis: &Genesis, ops: &[SignedOp]) -> AclState {
    replay_with_audit(genesis, ops).0
}

/// [`replay`] plus the per-op audit trail, in the same deterministic order.
pub fn replay_with_audit(genesis: &Genesis, ops: &[SignedOp]) -> (AclState, Vec<AuditEntry>) {
    // Index signature-valid ops by hash. Undecodable ops stay as opaque DAG
    // nodes (ancestry, no state) — the forward-compat rule in the module docs.
    let ws = genesis.workspace_id.as_str();
    let mut nodes: HashMap<String, &SignedOp> = HashMap::new();
    let mut decoded: HashMap<String, Option<AclOp>> = HashMap::new();
    for so in ops {
        if !so.verify_sig(ACL_DOMAIN, ws) {
            continue;
        }
        let h = so.hash();
        decoded.insert(h.clone(), postcard::from_bytes(&so.op).ok());
        nodes.insert(h, so);
    }

    let ancestors = sigdag::compute_ancestors(&nodes);
    let order = sigdag::topo_order(&nodes);

    // ---- pass 1 (topo): authorize ops, tracking standing as it evolves ----
    let mut admins: BTreeSet<UserId> = genesis.founding_admins.iter().cloned().collect();
    let mut humans: BTreeSet<UserId> = admins.clone();
    let mut agents_now: BTreeMap<UserId, UserId> = BTreeMap::new();

    let mut authorized: Vec<String> = Vec::new();
    let mut audit: Vec<AuditEntry> = Vec::new();
    for h in &order {
        let so = nodes[h];
        let op = &decoded[h];
        let mut entry = AuditEntry {
            hash: h.clone(),
            author: so.author.clone(),
            kind: "unknown",
            subject: None,
            role: None,
            authorized: false,
        };
        let Some(op) = op else {
            audit.push(entry); // opaque node: ancestry only
            continue;
        };
        entry.subject = Some(op.key().clone());
        entry.kind = match op {
            AclOp::AddMember { .. } => "add_member",
            AclOp::RemoveMember { .. } => "remove_member",
            AclOp::SetRole { .. } => "set_role",
            AclOp::AddAgent { .. } => "add_agent",
        };
        if let AclOp::AddMember { role, .. } | AclOp::SetRole { role, .. } = op {
            entry.role = Some(*role);
        }

        // Agents may author NO membership op (contract §3.4).
        let ok = !agents_now.contains_key(&so.author)
            && match op {
                AclOp::AddMember { .. } | AclOp::SetRole { .. } => admins.contains(&so.author),
                // Admins remove anyone; a sponsor may retire their own agent.
                AclOp::RemoveMember { key } => {
                    admins.contains(&so.author) || agents_now.get(key) == Some(&so.author)
                }
                // Any human member may sponsor an agent for themselves; the
                // agent key must be fresh (not already a principal).
                AclOp::AddAgent { key } => {
                    humans.contains(&so.author)
                        && key != &so.author
                        && !humans.contains(key)
                        && !agents_now.contains_key(key)
                }
            };
        entry.authorized = ok;
        audit.push(entry);
        if !ok {
            continue;
        }
        authorized.push(h.clone());
        match op {
            AclOp::AddMember { key, role } | AclOp::SetRole { key, role } => {
                humans.insert(key.clone());
                agents_now.remove(key);
                if *role == Role::Admin {
                    admins.insert(key.clone());
                } else {
                    admins.remove(key);
                }
            }
            AclOp::AddAgent { key } => {
                agents_now.insert(key.clone(), so.author.clone());
            }
            AclOp::RemoveMember { key } => {
                humans.remove(key);
                admins.remove(key);
                agents_now.remove(key);
                // in-pass sponsor cascade so an orphaned agent cannot author
                // (nothing to author anyway) nor be counted as standing.
                agents_now.retain(|_, sponsor| sponsor != key);
            }
        }
    }

    // ---- pass 2: materialize membership from authorized ops in topo order ----
    let mut members: BTreeMap<UserId, Role> = genesis
        .founding_admins
        .iter()
        .map(|a| (a.clone(), Role::Admin))
        .collect();
    let mut agents: BTreeMap<UserId, UserId> = BTreeMap::new();

    for h in &authorized {
        match decoded[h].as_ref().expect("authorized ops decoded") {
            AclOp::AddMember { key, role } | AclOp::SetRole { key, role } => {
                members.insert(key.clone(), *role);
                agents.remove(key);
            }
            AclOp::AddAgent { key } => {
                members.insert(key.clone(), Role::Member);
                agents.insert(key.clone(), nodes[h].author.clone());
            }
            AclOp::RemoveMember { key } => {
                members.remove(key);
                agents.remove(key);
            }
        }
    }

    // ---- remove-wins override (S§6): an authorized remove not causally
    // succeeded by an authorized (re-)add removes the key even if a concurrent
    // add appeared later in topo order. AddAgent counts as an add of its key.
    let keys: BTreeSet<UserId> = authorized
        .iter()
        .filter_map(|h| decoded[h].as_ref().map(|op| op.key().clone()))
        .collect();
    for key in keys {
        let adds: Vec<&String> = authorized
            .iter()
            .filter(|h| {
                matches!(decoded[*h].as_ref(),
                    Some(AclOp::AddMember { key: k, .. })
                    | Some(AclOp::SetRole { key: k, .. })
                    | Some(AclOp::AddAgent { key: k }) if k == &key)
            })
            .collect();
        let removes: Vec<&String> = authorized
            .iter()
            .filter(|h| {
                matches!(decoded[*h].as_ref(), Some(AclOp::RemoveMember { key: k }) if k == &key)
            })
            .collect();
        if removes.is_empty() {
            continue;
        }
        let removed = removes.iter().any(|r| {
            !adds.iter().any(|a| {
                ancestors
                    .get(*a)
                    .map(|anc| anc.contains(*r))
                    .unwrap_or(false)
            })
        });
        if removed {
            members.remove(&key);
            agents.remove(&key);
        }
    }

    // ---- sponsor cascade: an agent stands only while its sponsor does.
    // Sponsors are never agents (AddAgent authorization), so one pass suffices.
    let orphaned: Vec<UserId> = agents
        .iter()
        .filter(|(_, sponsor)| !members.contains_key(*sponsor))
        .map(|(k, _)| k.clone())
        .collect();
    for k in orphaned {
        agents.remove(&k);
        members.remove(&k);
    }

    (AclState { members, agents }, audit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;

    fn seed(n: u8) -> [u8; 32] {
        [n; 32]
    }
    fn user(n: u8) -> UserId {
        let pk = SigningKey::from_bytes(&seed(n)).verifying_key();
        UserId::from_key_string(data_encoding::HEXLOWER.encode(pk.as_bytes()))
    }
    fn genesis(admins: &[u8]) -> Genesis {
        Genesis {
            workspace_id: crate::ids::WorkspaceId::mint(&crate::ids::SystemUlidSource),
            founding_admins: admins.iter().map(|n| user(*n)).collect(),
        }
    }

    #[test]
    fn founder_is_admin_and_can_add_members() {
        let g = genesis(&[1]);
        let add = sign_op(
            &seed(1),
            &AclOp::AddMember {
                key: user(2),
                role: Role::Member,
            },
            vec![],
            &g.workspace_id,
        );
        let st = replay(&g, &[add]);
        assert!(st.is_admin(&user(1)));
        assert!(st.is_member(&user(2)));
        assert!(!st.is_admin(&user(2)));
        assert_eq!(st.len(), 2);
    }

    #[test]
    fn non_admin_ops_are_rejected() {
        let g = genesis(&[1]);
        // user 2 (not a member) tries to add user 3 — unauthorized, ignored.
        let forged = sign_op(
            &seed(2),
            &AclOp::AddMember {
                key: user(3),
                role: Role::Admin,
            },
            vec![],
            &g.workspace_id,
        );
        let st = replay(&g, &[forged]);
        assert!(
            !st.is_member(&user(3)),
            "an unauthorized op must not take effect"
        );
        assert!(!st.is_member(&user(2)));
    }

    #[test]
    fn forged_signature_is_rejected() {
        let g = genesis(&[1]);
        let mut op = sign_op(
            &seed(1),
            &AclOp::AddMember {
                key: user(2),
                role: Role::Member,
            },
            vec![],
            &g.workspace_id,
        );
        op.sig[0] ^= 0xff; // tamper
        let st = replay(&g, &[op]);
        assert!(!st.is_member(&user(2)), "a bad signature must be rejected");
    }

    #[test]
    fn remove_wins_over_concurrent_add() {
        // Founder adds B (op1). Then two concurrent branches off op1: admin
        // removes B (rm), and admin re-adds B (add2) — both children of op1, so
        // concurrent. Remove-wins ⇒ B is not a member.
        let g = genesis(&[1]);
        let op1 = sign_op(
            &seed(1),
            &AclOp::AddMember {
                key: user(2),
                role: Role::Member,
            },
            vec![],
            &g.workspace_id,
        );
        let h1 = op1.hash();
        let rm = sign_op(
            &seed(1),
            &AclOp::RemoveMember { key: user(2) },
            vec![h1.clone()],
            &g.workspace_id,
        );
        let add2 = sign_op(
            &seed(1),
            &AclOp::AddMember {
                key: user(2),
                role: Role::Member,
            },
            vec![h1.clone()],
            &g.workspace_id,
        );
        let st = replay(&g, &[op1, rm, add2]);
        assert!(!st.is_member(&user(2)), "remove-wins over a concurrent add");
    }

    #[test]
    fn re_add_after_remove_restores_membership() {
        // Sequential: add, remove, then re-add whose parent is the remove.
        let g = genesis(&[1]);
        let op1 = sign_op(
            &seed(1),
            &AclOp::AddMember {
                key: user(2),
                role: Role::Member,
            },
            vec![],
            &g.workspace_id,
        );
        let rm = sign_op(
            &seed(1),
            &AclOp::RemoveMember { key: user(2) },
            vec![op1.hash()],
            &g.workspace_id,
        );
        let readd = sign_op(
            &seed(1),
            &AclOp::AddMember {
                key: user(2),
                role: Role::Member,
            },
            vec![rm.hash()],
            &g.workspace_id,
        );
        let st = replay(&g, &[op1, rm, readd]);
        assert!(
            st.is_member(&user(2)),
            "a causally-later re-add restores membership"
        );
    }

    #[test]
    fn revocation_holds_against_reparented_signed_add() {
        // Regression for the validation-found CRITICAL: an evicted member (no
        // admin key) tries to defeat remove-wins by lifting the admin's still-
        // valid signed AddMember op and re-parenting the copy to descend from the
        // removal. Because the signature binds `parents` (+ workspace id), the
        // re-parented copy fails verification and is dropped by replay.
        let g = genesis(&[1]); // user1 = founding admin
        let add_orig = sign_op(
            &seed(1),
            &AclOp::AddMember {
                key: user(2),
                role: Role::Member,
            },
            vec![],
            &g.workspace_id,
        );
        let rm = sign_op(
            &seed(1),
            &AclOp::RemoveMember { key: user(2) },
            vec![add_orig.hash()],
            &g.workspace_id,
        );
        // Baseline: after add + remove, B is correctly gone.
        let base = replay(&g, &[add_orig.clone(), rm.clone()]);
        assert!(!base.is_member(&user(2)), "baseline: B removed");

        // ATTACK — reuse the admin's op bytes/author/sig verbatim; only mutate the
        // `parents` to point AFTER the removal.
        let add_replay = SignedOp {
            op: add_orig.op.clone(),
            author: add_orig.author.clone(),
            sig: add_orig.sig.clone(),
            parents: vec![rm.hash()],
        };
        assert!(
            !add_replay.verify_sig(ACL_DOMAIN, g.workspace_id.as_str()),
            "re-parented copy must FAIL signature verification (sig binds parents)"
        );
        let st = replay(&g, &[add_orig, rm, add_replay]);
        assert!(
            !st.is_member(&user(2)),
            "revocation must hold: the re-parented op is rejected and B stays removed"
        );
    }

    #[test]
    fn op_from_another_workspace_is_rejected() {
        // Regression: a valid op signed for workspace A must not be honored when
        // replayed against workspace B's genesis (the signature binds the ws id).
        let g_a = genesis(&[1]);
        let mut g_b = genesis(&[1]);
        // Distinct workspace id, same founding admin key.
        while g_b.workspace_id == g_a.workspace_id {
            g_b.workspace_id = crate::ids::WorkspaceId::mint(&crate::ids::SystemUlidSource);
        }
        let op = sign_op(
            &seed(1),
            &AclOp::AddMember {
                key: user(2),
                role: Role::Admin,
            },
            vec![],
            &g_a.workspace_id, // signed for A
        );
        let st_b = replay(&g_b, &[op]);
        assert!(
            !st_b.is_member(&user(2)),
            "an op signed for workspace A must not take effect in workspace B"
        );
    }

    #[test]
    fn replay_is_order_independent() {
        let g = genesis(&[1]);
        let op1 = sign_op(
            &seed(1),
            &AclOp::AddMember {
                key: user(2),
                role: Role::Admin,
            },
            vec![],
            &g.workspace_id,
        );
        let op2 = sign_op(
            &seed(2), // user 2 is now an admin
            &AclOp::AddMember {
                key: user(3),
                role: Role::Member,
            },
            vec![op1.hash()],
            &g.workspace_id,
        );
        let a = replay(&g, &[op1.clone(), op2.clone()]);
        let b = replay(&g, &[op2, op1]);
        assert_eq!(a, b, "replay is deterministic regardless of delivery order");
        assert!(a.is_member(&user(3)));
    }

    // ---- agents (contract §3.4) ----

    #[test]
    fn member_sponsors_agent_and_agent_is_sealed_but_powerless() {
        let g = genesis(&[1]);
        let add_b = sign_op(
            &seed(1),
            &AclOp::AddMember {
                key: user(2),
                role: Role::Member,
            },
            vec![],
            &g.workspace_id,
        );
        // B (a plain member) sponsors agent X.
        let add_agent = sign_op(
            &seed(2),
            &AclOp::AddAgent { key: user(10) },
            vec![add_b.hash()],
            &g.workspace_id,
        );
        // X (an agent) tries to add member Y — never authorized.
        let agent_forges = sign_op(
            &seed(10),
            &AclOp::AddMember {
                key: user(11),
                role: Role::Member,
            },
            vec![add_agent.hash()],
            &g.workspace_id,
        );
        // X tries to sponsor its own agent — no re-delegation.
        let agent_delegates = sign_op(
            &seed(10),
            &AclOp::AddAgent { key: user(12) },
            vec![add_agent.hash()],
            &g.workspace_id,
        );
        let st = replay(&g, &[add_b, add_agent, agent_forges, agent_delegates]);
        assert!(st.is_member(&user(10)), "the agent is sealed (a member)");
        assert!(st.is_agent(&user(10)));
        assert_eq!(st.sponsor_of(&user(10)), Some(&user(2)));
        assert!(!st.is_human_member(&user(10)));
        assert!(!st.is_member(&user(11)), "agent membership ops are void");
        assert!(!st.is_member(&user(12)), "agents cannot sponsor agents");
        assert_eq!(st.standing(&user(10)), Some("agent"));
        assert_eq!(st.standing(&user(2)), Some("member"));
    }

    #[test]
    fn non_member_cannot_sponsor_and_agent_key_must_be_fresh() {
        let g = genesis(&[1]);
        // A stranger sponsors an agent — void.
        let stranger = sign_op(
            &seed(5),
            &AclOp::AddAgent { key: user(10) },
            vec![],
            &g.workspace_id,
        );
        // The founder "sponsors" an existing human as an agent — void.
        let add_b = sign_op(
            &seed(1),
            &AclOp::AddMember {
                key: user(2),
                role: Role::Member,
            },
            vec![],
            &g.workspace_id,
        );
        let demote = sign_op(
            &seed(1),
            &AclOp::AddAgent { key: user(2) },
            vec![add_b.hash()],
            &g.workspace_id,
        );
        // Self-sponsoring is void.
        let selfie = sign_op(
            &seed(1),
            &AclOp::AddAgent { key: user(1) },
            vec![],
            &g.workspace_id,
        );
        let st = replay(&g, &[stranger, add_b.clone(), demote, selfie]);
        assert!(!st.is_member(&user(10)));
        assert!(!st.is_agent(&user(2)), "a human cannot be demoted to agent");
        assert!(!st.is_agent(&user(1)));
    }

    #[test]
    fn removing_the_sponsor_cascades_to_their_agents() {
        let g = genesis(&[1]);
        let add_b = sign_op(
            &seed(1),
            &AclOp::AddMember {
                key: user(2),
                role: Role::Member,
            },
            vec![],
            &g.workspace_id,
        );
        let add_agent = sign_op(
            &seed(2),
            &AclOp::AddAgent { key: user(10) },
            vec![add_b.hash()],
            &g.workspace_id,
        );
        let rm_b = sign_op(
            &seed(1),
            &AclOp::RemoveMember { key: user(2) },
            vec![add_agent.hash()],
            &g.workspace_id,
        );
        let st = replay(&g, &[add_b, add_agent, rm_b]);
        assert!(!st.is_member(&user(2)), "sponsor removed");
        assert!(
            !st.is_member(&user(10)),
            "the agent's membership dies with its sponsor"
        );
        assert!(st.agents().is_empty());
    }

    #[test]
    fn sponsor_can_retire_their_own_agent_but_not_other_members() {
        let g = genesis(&[1]);
        let add_b = sign_op(
            &seed(1),
            &AclOp::AddMember {
                key: user(2),
                role: Role::Member,
            },
            vec![],
            &g.workspace_id,
        );
        let add_c = sign_op(
            &seed(1),
            &AclOp::AddMember {
                key: user(3),
                role: Role::Member,
            },
            vec![add_b.hash()],
            &g.workspace_id,
        );
        let add_agent = sign_op(
            &seed(2),
            &AclOp::AddAgent { key: user(10) },
            vec![add_c.hash()],
            &g.workspace_id,
        );
        // B retires their own agent — authorized without admin.
        let retire = sign_op(
            &seed(2),
            &AclOp::RemoveMember { key: user(10) },
            vec![add_agent.hash()],
            &g.workspace_id,
        );
        // B (not an admin) tries to remove member C — void.
        let overreach = sign_op(
            &seed(2),
            &AclOp::RemoveMember { key: user(3) },
            vec![retire.hash()],
            &g.workspace_id,
        );
        let st = replay(&g, &[add_b, add_c, add_agent, retire, overreach]);
        assert!(!st.is_member(&user(10)), "sponsor retired their agent");
        assert!(
            st.is_member(&user(3)),
            "a member cannot remove other members"
        );
    }

    #[test]
    fn undecodable_ops_hold_ancestry_without_state() {
        // Forward compat (module docs): a signature-valid op with unknown bytes
        // is an opaque DAG node. Here: add(B) → OPAQUE → remove(B) → re-add(B)
        // parented on the opaque node's CHILD chain; the re-add must still be
        // recognized as causally succeeding the remove (ancestry intact), so B
        // is a member — a build that dropped the opaque node would break the
        // chain and remove-wins would falsely evict B.
        let g = genesis(&[1]);
        let add = sign_op(
            &seed(1),
            &AclOp::AddMember {
                key: user(2),
                role: Role::Member,
            },
            vec![],
            &g.workspace_id,
        );
        let rm = sign_op(
            &seed(1),
            &AclOp::RemoveMember { key: user(2) },
            vec![add.hash()],
            &g.workspace_id,
        );
        // An op kind this build does not know: raw bytes that fail decode but
        // carry a valid signature, causally after the remove.
        let opaque = sigdag::sign_node(
            ACL_DOMAIN,
            &seed(1),
            vec![0xff, 0xff, 0xff, 0xff],
            vec![rm.hash()],
            g.workspace_id.as_str(),
        );
        let readd = sign_op(
            &seed(1),
            &AclOp::AddMember {
                key: user(2),
                role: Role::Member,
            },
            vec![opaque.hash()],
            &g.workspace_id,
        );
        let st = replay(&g, &[add, rm, opaque, readd]);
        assert!(
            st.is_member(&user(2)),
            "ancestry must flow through opaque nodes: the re-add causally succeeds the remove"
        );
    }

    #[test]
    fn audit_log_renders_verdicts_in_causal_order() {
        let g = genesis(&[1]);
        let add = sign_op(
            &seed(1),
            &AclOp::AddMember {
                key: user(2),
                role: Role::Member,
            },
            vec![],
            &g.workspace_id,
        );
        let forged = sign_op(
            &seed(3),
            &AclOp::RemoveMember { key: user(2) },
            vec![add.hash()],
            &g.workspace_id,
        );
        let (st, audit) = replay_with_audit(&g, &[add, forged]);
        assert!(st.is_member(&user(2)));
        assert_eq!(audit.len(), 2);
        assert_eq!(audit[0].kind, "add_member");
        assert!(audit[0].authorized);
        assert_eq!(audit[1].kind, "remove_member");
        assert!(
            !audit[1].authorized,
            "the forged remove is logged, not honored"
        );
    }
}
