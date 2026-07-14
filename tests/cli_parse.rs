//! Argv → `Request` parity guards for the programmatic-clap surface
//! (`src/cmdspec.rs`). The command tree is now data, not a `#[derive(Parser)]`
//! enum, and arg extraction is keyed by string inside each spec's closure — so a
//! renamed arg is a runtime, not compile-time, error. These tests pin the tricky
//! mappings (label +/- tokens, board-position flags, repeated/variadic args,
//! aliases, defaults, and parse-level conflicts) that the derive used to enforce.
//!
//! Comparison is by serde value so we assert the *whole* `Request`, tag and all,
//! without depending on its wire representation.

use lait::cmdspec::{build_cli, parse_to_request, specs};
use lait::control::{BoardPos, Filter, Request};
use serde_json::to_value;

/// Assert an argv parses to exactly `expected`.
fn parses_to(argv: &[&str], expected: Request) {
    let got = parse_to_request(argv).unwrap_or_else(|e| panic!("parse {argv:?}: {e}"));
    assert_eq!(
        to_value(&got).unwrap(),
        to_value(&expected).unwrap(),
        "argv {argv:?} produced the wrong Request",
    );
}

#[test]
fn label_tokens_split_into_add_and_remove() {
    // `+bug` adds, `-wip` removes, a bare token adds — and `-wip` must survive as a
    // value (allow_hyphen_values), not be parsed as an unknown flag.
    parses_to(
        &["lait", "label", "ENG-1", "+bug", "-wip", "chore"],
        Request::Label {
            reff: "ENG-1".into(),
            add: vec!["bug".into(), "chore".into()],
            remove: vec!["wip".into()],
        },
    );
}

#[test]
fn move_position_flags_map_to_boardpos() {
    parses_to(
        &["lait", "move", "ENG-1", "--top"],
        Request::IssueMove {
            reff: "ENG-1".into(),
            project: None,
            pos: Some(BoardPos::Top),
        },
    );
    parses_to(
        &["lait", "move", "ENG-1", "--before", "ENG-2", "-p", "ENG"],
        Request::IssueMove {
            reff: "ENG-1".into(),
            project: Some("ENG".into()),
            pos: Some(BoardPos::Before {
                reff: "ENG-2".into(),
            }),
        },
    );
}

#[test]
fn assign_collects_variadic_who_and_toggles_add() {
    parses_to(
        &["lait", "assign", "ENG-1", "alice", "bob", "--remove"],
        Request::Assign {
            reff: "ENG-1".into(),
            who: vec!["alice".into(), "bob".into()],
            add: false,
        },
    );
    parses_to(
        &["lait", "assign", "ENG-1", "alice"],
        Request::Assign {
            reff: "ENG-1".into(),
            who: vec!["alice".into()],
            add: true,
        },
    );
}

#[test]
fn new_collects_repeated_short_flags() {
    parses_to(
        &[
            "lait",
            "new",
            "Fix login",
            "-p",
            "ENG",
            "-a",
            "alice",
            "-a",
            "bob",
            "-l",
            "x",
            "-l",
            "y",
            "-P",
            "high",
            "-b",
            "details",
        ],
        Request::IssueNew {
            title: "Fix login".into(),
            project: Some("ENG".into()),
            assignees: vec!["alice".into(), "bob".into()],
            priority: Some("high".into()),
            labels: vec!["x".into(), "y".into()],
            body: Some("details".into()),
        },
    );
}

#[test]
fn ls_filter_flags() {
    parses_to(
        &[
            "lait", "ls", "-p", "ENG", "--mine", "--status", "wip", "--all",
        ],
        Request::List {
            project: Some("ENG".into()),
            filter: Filter {
                mine: true,
                status: Some("wip".into()),
                label: None,
                all: true,
            },
        },
    );
}

#[test]
fn activity_since_defaults_to_zero() {
    parses_to(&["lait", "activity"], Request::Activity { since: 0 });
    parses_to(
        &["lait", "activity", "--since", "42"],
        Request::Activity { since: 42 },
    );
}

#[test]
fn comment_with_inline_body() {
    // (The stdin fallback path is intentionally not exercised — it would block.)
    parses_to(
        &["lait", "comment", "ENG-1", "looks good"],
        Request::Comment {
            reff: "ENG-1".into(),
            body: "looks good".into(),
        },
    );
}

#[test]
fn aliases_resolve_to_the_canonical_command() {
    // `verify` → `doctor`, `seed ls` → `remote ls`, `members alias` → `members name`.
    parses_to(
        &["lait", "verify"],
        Request::Diagnose {
            expected_workspace: None,
        },
    );
    parses_to(&["lait", "seed", "ls"], Request::SeedList);
    parses_to(
        &["lait", "members", "alias", "abc123", "Alice"],
        Request::MemberAlias {
            who: "abc123".into(),
            name: "Alice".into(),
        },
    );
}

#[test]
fn grouped_commands_bare_form_lists() {
    parses_to(&["lait", "projects"], Request::ProjectList);
    parses_to(&["lait", "labels"], Request::LabelList);
    parses_to(&["lait", "members"], Request::Members);
}

#[test]
fn members_add_reads_admin_and_local_name() {
    parses_to(
        &["lait", "members", "add", "abc", "--admin", "--as", "Alice"],
        Request::MemberAdd {
            who: "abc".into(),
            admin: true,
            as_name: Some("Alice".into()),
        },
    );
}

#[test]
fn invite_require_approval_conflicts_with_pass_tuning() {
    // A parse-level conflict the derive enforced with conflicts_with_all — must
    // still be rejected before dispatch.
    let cli = build_cli(&specs());
    let res = cli.try_get_matches_from(["lait", "invite", "--require-approval", "--reusable"]);
    assert!(
        res.is_err(),
        "--require-approval with --reusable should be a usage error",
    );
}

#[test]
fn unknown_subcommand_is_a_usage_error() {
    assert!(parse_to_request(&["lait", "frobnicate"]).is_err());
}
