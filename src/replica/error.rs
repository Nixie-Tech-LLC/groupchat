//! Why a replica operation failed, as structured values.
//!
//! The replica is the domain: it decides what is legitimate, not how to say so.
//! An error here names *what went wrong* and carries the data that made it go
//! wrong; turning that into a sentence a person reads — and into a wire
//! `Response` — belongs to the control adapter in [`super::dispatch`], which is
//! the single door between this crate's domain and the client protocol.
//!
//! Two consequences worth stating, because they are the reason the type exists:
//! a caller can distinguish "no such ref" from "you may not do that" without
//! matching on prose, and the domain no longer depends on the control plane, so
//! it can be lifted out from under the daemon without dragging it along.
//!
//! **On `Display`.** These render exactly the sentences the CLI, web, and MCP
//! surfaces already show. That is deliberate: this refactor changes where a
//! message is produced, never what a person sees. Wording changes are their own
//! commits, with their own reasons.

use std::fmt;

/// A replica operation's failure.
#[derive(Debug)]
pub enum ReplicaError {
    /// Nothing here answers to that name. The only family the control plane
    /// reports as `NotFound` (exit code 2), so a script can tell "absent" from
    /// "refused" without reading the message.
    NotFound(NotFound),
    /// The caller may not do this — membership standing, admin gates, or a
    /// self-protection rule.
    Denied(Denied),
    /// The request itself is malformed: empty where a value is required, or not
    /// the shape an id/key/blob must take.
    Invalid(Invalid),
    /// Well-formed and permitted, but it contradicts state that already exists.
    Conflict(Conflict),
    /// A multi-step ceremony (recovery, elevation, custody) refused a step.
    /// Boxed: this is the widest variant by far and every `Result` in the
    /// replica would otherwise pay for its size.
    Ceremony(Box<Ceremony>),
    /// A failure from beneath the domain — persistence, encoding, crypto —
    /// carried verbatim. Formatted with the anyhow chain, as it always was.
    Internal(anyhow::Error),
}

/// Something was named that does not exist here.
#[derive(Debug)]
pub enum NotFound {
    Project { named: String },
    Label { named: String },
}

/// The caller lacks the standing this operation requires.
#[derive(Debug)]
pub enum Denied {
    /// A member holding neither Write nor Admin — a viewer — tried to mutate
    /// space content.
    ViewOnly,
    /// An admin-gated membership operation, attempted without admin.
    NotAdmin(AdminAction),
    /// Agents hold no membership authority of their own.
    NotHuman,
    /// Removing yourself would strand the space; leaving is a different verb.
    SelfRemoval,
    /// This device has not established an actor identity yet.
    NoActorIdentity { in_this_space: bool },
}

/// The admin-gated operations, named so the message can say which was refused.
#[derive(Debug, Clone, Copy)]
pub enum AdminAction {
    AddMember,
    RemoveMember,
    RevokeInvite,
    RotateKey,
    DeleteIssue,
}

/// The request could not be understood.
#[derive(Debug)]
pub enum Invalid {
    /// A field that must carry text was empty.
    Empty { field: &'static str },
    /// A value was not the shape it must take (an id, a key, a blob).
    Malformed { what: &'static str },
    /// An edit that names no change.
    NothingToEdit,
}

/// The operation contradicts state that already exists.
#[derive(Debug)]
pub enum Conflict {
    InviteRedeemed,
    InviteRevoked,
    /// An issue-graph edge that would make a cycle or a self-reference.
    IssueGraph(GraphViolation),
}

#[derive(Debug, Clone, Copy)]
pub enum GraphViolation {
    SelfParent,
    SelfLink,
    Ancestor,
}

/// A ceremony step refused. These carry the operator-facing guidance the
/// original messages carried — a ceremony failure is usually actionable, and
/// the action is rarely obvious.
#[derive(Debug)]
pub enum Ceremony {
    /// Free-form ceremony refusal, carrying its own guidance verbatim. The
    /// ceremony surface is wide and mostly one-of-a-kind; structuring every
    /// distinct refusal would produce a variant per message and buy nothing.
    Refused { message: String },
}

impl ReplicaError {
    /// Convenience for the many ceremony refusals that are one-of-a-kind.
    pub fn ceremony(message: impl Into<String>) -> Self {
        Self::Ceremony(Box::new(Ceremony::Refused {
            message: message.into(),
        }))
    }
}

impl From<anyhow::Error> for ReplicaError {
    fn from(e: anyhow::Error) -> Self {
        Self::Internal(e)
    }
}

impl fmt::Display for ReplicaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(e) => e.fmt(f),
            Self::Denied(e) => e.fmt(f),
            Self::Invalid(e) => e.fmt(f),
            Self::Conflict(e) => e.fmt(f),
            Self::Ceremony(e) => e.fmt(f),
            Self::Internal(e) => write!(f, "{e:#}"),
        }
    }
}

impl fmt::Display for NotFound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Project { named } => write!(f, "no project matches '{named}'"),
            Self::Label { named } => write!(f, "no label matches '{named}'"),
        }
    }
}

impl fmt::Display for Denied {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ViewOnly => f.write_str("view-only: your membership grants no write access"),
            Self::NotAdmin(action) => match action {
                AdminAction::AddMember => f.write_str("only an admin can add members"),
                AdminAction::RemoveMember => f.write_str("only an admin can remove members"),
                AdminAction::RevokeInvite => f.write_str("only an admin can revoke an invite"),
                AdminAction::RotateKey => f.write_str("only an admin can rotate the key"),
                AdminAction::DeleteIssue => {
                    f.write_str("no content authority to delete issues (view-only or agent)")
                }
            },
            Self::NotHuman => f.write_str("only a human member can sponsor an agent"),
            Self::SelfRemoval => f.write_str("refusing to remove yourself"),
            Self::NoActorIdentity { in_this_space } => {
                if *in_this_space {
                    f.write_str("this device has no actor identity in this space yet")
                } else {
                    f.write_str("this device has no actor identity")
                }
            }
        }
    }
}

impl fmt::Display for Invalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty { field } => match *field {
                "title" => f.write_str("title must not be empty"),
                "comment" => f.write_str("comment body must not be empty"),
                "label" => f.write_str("label name is required"),
                "project" => f.write_str("project name and key are required"),
                other => write!(f, "{other} must not be empty"),
            },
            Self::Malformed { what } => f.write_str(what),
            Self::NothingToEdit => f.write_str("nothing to edit"),
        }
    }
}

impl fmt::Display for Conflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InviteRedeemed => f.write_str("invite already redeemed"),
            Self::InviteRevoked => f.write_str("this invite has been revoked"),
            Self::IssueGraph(v) => match v {
                GraphViolation::SelfParent => f.write_str("an issue cannot be its own parent"),
                GraphViolation::SelfLink => f.write_str("an issue cannot link to itself"),
                GraphViolation::Ancestor => {
                    f.write_str("that would make an issue its own ancestor")
                }
            },
        }
    }
}

impl fmt::Display for Ceremony {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused { message } => f.write_str(message),
        }
    }
}
