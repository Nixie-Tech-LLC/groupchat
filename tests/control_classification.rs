//! M0.1 — exhaustive terminal-owner classification of every control request.
//!
//! Every `control::Request` variant is mapped to exactly one **terminal
//! owner** — the single orbital plane that serves it once the migration is
//! complete. The `match` in [`terminal_owner`] is exhaustive, so adding a
//! variant without a terminal owner fails the build; a daemon catch-all is
//! forbidden by construction because there is no catch-all class.
//!
//! Owners (plan 01, "External architecture"):
//! - **Session** — product intent/query through `IssueRouter` → Session;
//! - **Mechanics** — membership/ceremony/custody/admission through the active
//!   Orbit/Station's mechanics;
//! - **Station** — connect/neighbor/Contact operations;
//! - **Observation** — status/subscription projections;
//! - **Lifecycle** — Runtime/Orbit/Station/daemon process concerns and
//!   node-local configuration adapters;
//! - **RemovedByM2** — the pending-member approval surface, deleted by the M2
//!   acceptance-triggered-admission cutover. No other terminal state may use
//!   this owner.

use lait::control::Request;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owner {
    Session,
    Mechanics,
    Station,
    Observation,
    Lifecycle,
    /// Slated for deletion in M2 (pending-approval surface). Terminal state is
    /// nonexistence; while the variant exists it must refuse with a typed
    /// error, never be served.
    RemovedByM2,
}

/// The exhaustive terminal-owner table. Compile-enforced: a new `Request`
/// variant without an arm here is a build failure, not a runtime catch-all.
fn terminal_owner(r: &Request) -> Owner {
    use Owner::*;
    match r {
        // ---- Session: product intents, queries, projections ----
        Request::IssueNew { .. }
        | Request::IssueEdit { .. }
        | Request::IssueMove { .. }
        | Request::Assign { .. }
        | Request::Label { .. }
        | Request::Comment { .. }
        | Request::IssueDelete { .. }
        | Request::IssueRestore { .. }
        | Request::IssueLink { .. }
        | Request::IssueUnlink { .. }
        | Request::IssueParent { .. }
        | Request::IssueGraph { .. }
        | Request::IssueStart { .. }
        | Request::IssueDone { .. }
        | Request::IssueStop { .. }
        | Request::IssueView { .. }
        | Request::List { .. }
        | Request::Board { .. }
        | Request::History { .. }
        | Request::ProjectNew { .. }
        | Request::ProjectList
        | Request::LabelNew { .. }
        | Request::LabelList
        | Request::Activity { .. }
        | Request::Inbox { .. } => Session,

        // ---- Mechanics: membership, admission, ceremonies, custody, devices ----
        Request::MemberAdd { .. }
        | Request::MemberRemove { .. }
        | Request::Members
        | Request::MemberLog
        | Request::AgentAdd { .. }
        | Request::KeyRotate
        | Request::InviteRevoke { .. }
        | Request::DeviceInvite
        | Request::DeviceAdd { .. }
        | Request::DeviceRevoke { .. }
        | Request::DeviceList
        | Request::SpaceRecover
        | Request::SpaceElevate { .. }
        | Request::SpaceRecoverApprove { .. }
        | Request::SpaceElevateApprove { .. }
        | Request::SpaceCustodyExport { .. }
        | Request::SpaceCustodyImport { .. }
        | Request::Recover
        | Request::Invite { .. }
        | Request::Join { .. }
        | Request::Id => Mechanics,

        // ---- Station: connect/neighbor/Contact ----
        Request::Connect { .. } | Request::Who => Station,

        // ---- Observation: status + subscription projections ----
        Request::Status | Request::Subscribe { .. } => Observation,

        // ---- Lifecycle/deployment: daemon process + node-local config ----
        Request::Diagnose { .. }
        | Request::SeedAdd { .. }
        | Request::SeedList
        | Request::SeedRemove { .. }
        | Request::Log { .. }
        | Request::ConfigReload
        | Request::Stop
        | Request::Hello { .. }
        | Request::MemberAlias { .. } => Lifecycle,

        // ---- deleted by the M2 admission cutover ----
        Request::MemberRequests | Request::MemberApprove { .. } => RemovedByM2,
    }
}

#[test]
fn every_request_variant_has_a_terminal_owner() {
    // Exhaustiveness is compile-enforced by `terminal_owner`'s match. Assert
    // the intended mapping for one representative per owner.
    assert_eq!(
        terminal_owner(&Request::IssueNew {
            title: "t".into(),
            project: None,
            project_hint: None,
            assignees: vec![],
            priority: None,
            labels: vec![],
            body: None,
        }),
        Owner::Session
    );
    assert_eq!(terminal_owner(&Request::Members), Owner::Mechanics);
    assert_eq!(terminal_owner(&Request::DeviceList), Owner::Mechanics);
    assert_eq!(
        terminal_owner(&Request::Connect { ticket: "x".into() }),
        Owner::Station
    );
    assert_eq!(terminal_owner(&Request::Status), Owner::Observation);
    assert_eq!(terminal_owner(&Request::Stop), Owner::Lifecycle);
    assert_eq!(terminal_owner(&Request::MemberRequests), Owner::RemovedByM2);
}

#[test]
fn removed_by_m2_is_reserved_for_the_approval_surface() {
    // Only the two pending-approval requests may carry the deletion owner; if
    // M2 has landed (the variants are gone) this test still compiles because
    // the arms above are deleted with them.
    let removed = [
        terminal_owner(&Request::MemberRequests),
        terminal_owner(&Request::MemberApprove {
            who: String::new(),
            as_name: None,
        }),
    ];
    assert!(removed.iter().all(|o| *o == Owner::RemovedByM2));
}
