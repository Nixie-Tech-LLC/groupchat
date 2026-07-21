//! G8 — exhaustive classification of every current control request into one of
//! the four orbital classes. The `match` is exhaustive, so the **compiler**
//! guarantees no `Request` variant is left unclassified: adding a variant
//! without classifying it fails the build. This is the table S1 requires before
//! the product is routed through Sessions in S5.
//!
//! Classes:
//! - **Lifecycle** — Space authority, membership, custody, keys, identity, and
//!   join/admission. In the orbital model these are mechanics/Orbit concerns
//!   (`form_space`/`enter_orbit`/authority), not World application meaning.
//! - **Deployment** — daemon/IPC/transport/process/seed/local-config concerns
//!   that become deployment adapters, never generic-runtime API.
//! - **Application** — World application intents, queries, and observation over
//!   the Issues product; these route through a Session in S5.
//! - **TemporaryAdapter** — bridges that exist only until the carve replaces
//!   them, kept behind the product adapter and removed in S6.

use lait::control::Request;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Lifecycle,
    Deployment,
    Application,
    TemporaryAdapter,
}

fn classify(r: &Request) -> Class {
    use Class::*;
    match r {
        // ---- Application: World intents, queries, projections, observation ----
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
        | Request::Inbox { .. }
        | Request::Who
        | Request::Subscribe { .. } => Application,

        // ---- Lifecycle: Space authority, membership, custody, keys, join ----
        Request::MemberAdd { .. }
        | Request::MemberRemove { .. }
        | Request::MemberApprove { .. }
        | Request::MemberRequests
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
        | Request::Connect { .. }
        | Request::Id => Lifecycle,

        // ---- Deployment: daemon/transport/process/local-node concerns ----
        Request::Status
        | Request::Diagnose { .. }
        | Request::SeedAdd { .. }
        | Request::SeedList
        | Request::SeedRemove { .. }
        | Request::Log { .. }
        | Request::ConfigReload
        | Request::Stop
        | Request::Hello { .. }
        // A local, never-synced petname: node-local state, a deployment concern.
        | Request::MemberAlias { .. } => Deployment,
    }
}

#[test]
fn every_request_variant_is_classified() {
    // Spot-check one representative of each class. Exhaustiveness itself is
    // enforced by the compiler on `classify`'s match; this asserts the mapping
    // is the intended one for a sample of each bucket.
    assert_eq!(
        classify(&Request::IssueNew {
            title: "t".into(),
            project: None,
            project_hint: None,
            assignees: vec![],
            priority: None,
            labels: vec![],
            body: None,
        }),
        Class::Application
    );
    assert_eq!(classify(&Request::Members), Class::Lifecycle);
    assert_eq!(
        classify(&Request::Join { ticket: "x".into() }),
        Class::Lifecycle
    );
    assert_eq!(classify(&Request::Status), Class::Deployment);
    assert_eq!(classify(&Request::Stop), Class::Deployment);
    assert_eq!(
        classify(&Request::Hello {
            protocol_version: 2
        }),
        Class::Deployment
    );
}

#[test]
fn temporary_adapter_class_exists_for_future_use() {
    // No current request is a pure temporary adapter, but the class is reserved
    // so S5/S6 can retire bridge requests without reshaping this taxonomy.
    let _ = Class::TemporaryAdapter;
}
