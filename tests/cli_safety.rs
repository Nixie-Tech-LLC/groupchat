//! Guards for the two promises the CLI makes before it does something you can't
//! undo, and before it blames the wrong component.
//!
//! 1. **Ask before destroying.** `delete` takes its ref from the git branch when
//!    you omit it, so it is the one verb that can tombstone something you never
//!    named. It must refuse rather than guess — and, with no terminal to ask on
//!    (CI, an agent, a pipe), it must refuse *without blocking* and name `--yes`.
//!    A prompt that hangs a CI job is worse than no prompt at all.
//!
//! 2. **Tell a foreign daemon from an absent one.** A daemon that is listening but
//!    speaks a different wire shape (an older lait still running after an upgrade)
//!    used to be reported as "no daemon" — which spawned a doomed second daemon
//!    over the held lock and waited out a 20s timeout before blaming the timeout.
//!    Detection is at the transport level, so this stays true across wire changes.
//!
//! 3. **Report failures in one voice.** Every client-side error goes through the
//!    top-level reporter: one lowercase `error:` line, the versioned DTO under
//!    `--json`, and the documented exit code. `main` returning `Result` used to
//!    hand these to anyhow's `Termination`, which broke all three.

use std::process::Command;
use std::time::{Duration, Instant};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_lait")
}

/// A short-lived home. Kept short on purpose: the control socket lives inside it
/// on unix and `sun_path` caps at 104 bytes (100 here), so a long temp path would
/// silently push the socket to the hashed temp-dir fallback.
fn tmp_home(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("lt-{}-{}", tag, std::process::id()));
    std::fs::remove_dir_all(&d).ok();
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The per-test config root. `$LAIT_HOME` isolates the *store*, but the spaces
/// registry lives under the config root — so without this every `init` here files
/// itself in the developer's real `lait spaces` list and never leaves.
fn config_root(home: &std::path::Path) -> std::path::PathBuf {
    home.join("cfg")
}

fn lait(home: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(bin())
        .env("LAIT_CONFIG_ROOT", config_root(home))
        // Every other integration suite pins this, and so does the CI smoke: a
        // daemon auto-spawned for a one-off command otherwise lingers for the
        // 30-minute idle window, and a client that connects while one is tearing
        // down can park (see `node::run_daemon`). Tests must not race that.
        .env("LAIT_IDLE_SECS", "0")
        .arg("--home")
        .arg(home)
        .args(args)
        .output()
        .expect("spawn lait")
}

fn init(home: &std::path::Path) {
    let out = lait(home, &["init", "--name", "t", "--nick", "t"]);
    assert!(out.status.success(), "init failed: {out:?}");
}

fn shutdown(home: &std::path::Path) {
    lait(home, &["shutdown"]);
}

#[test]
fn delete_without_yes_refuses_and_keeps_the_issue() {
    let home = tmp_home("del");
    init(&home);

    let out = lait(&home, &["new", "keep me"]);
    assert!(out.status.success(), "new failed: {out:?}");

    // `cargo test` gives the child no terminal — the CI/agent shape exactly.
    let started = Instant::now();
    let out = lait(&home, &["delete", "T-1"]);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        started.elapsed() < Duration::from_secs(10),
        "delete blocked waiting for input with no terminal to read from",
    );
    assert!(!out.status.success(), "delete must not proceed unconfirmed");
    assert!(
        stderr.contains("--yes"),
        "refusing is only half the job — it must name the flag that works \
         non-interactively; got: {stderr}",
    );
    // Naming the title is the point: on an inferred ref, "delete T-1?" is
    // unanswerable if you don't recall which issue T-1 is.
    assert!(
        stderr.contains("keep me"),
        "the prompt must name what it would destroy; got: {stderr}",
    );

    let out = lait(&home, &["ls"]);
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("keep me"),
        "the issue must survive an unconfirmed delete",
    );

    // ...and `--yes` is the way through.
    let out = lait(&home, &["--yes", "delete", "T-1"]);
    assert!(out.status.success(), "--yes must confirm: {out:?}");
    let out = lait(&home, &["ls"]);
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("keep me"),
        "--yes must actually delete",
    );

    shutdown(&home);
    std::fs::remove_dir_all(&home).ok();
}

/// A selector that matches nothing is a not-found, and must answer like one on
/// every channel: prose shape, `--json` DTO, and exit code.
#[test]
fn a_client_side_error_keeps_the_cli_contract() {
    let home = tmp_home("err");
    init(&home);

    // `-w` and `--home` are declared conflicting, so the home rides the env here
    // (the same channel `--home` sets internally).
    let run = |args: &[&str]| {
        Command::new(bin())
            .env("LAIT_HOME", &home)
            .env("LAIT_CONFIG_ROOT", config_root(&home))
            .env("LAIT_IDLE_SECS", "0")
            .args(args)
            .output()
            .expect("spawn lait")
    };

    let out = run(&["-w", "nosuchspace", "ls"]);
    let stderr = String::from_utf8_lossy(&out.stderr);

    // anyhow's Termination printed `Error:` (capitalised, Debug) while the daemon
    // path printed `error:` — two voices in one binary.
    assert!(
        stderr.starts_with("error:"),
        "errors must use the lowercase `error:` voice; got: {stderr}",
    );
    assert!(
        !stderr.contains("Caused by:"),
        "the cause chain is anyhow's Debug output, not a CLI contract; got: {stderr}",
    );
    // UI.md §2.3: 2 = not found / ambiguous. Termination made this a flat 1.
    assert_eq!(
        out.status.code(),
        Some(2),
        "a selector matching nothing must exit 2; stderr: {stderr}",
    );

    // `--json` is a contract: a consumer must get the DTO on stdout, not prose on
    // stderr and an empty stdout it can't distinguish from an empty result.
    let out = run(&["--json", "-w", "nosuchspace", "ls"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("stdout not JSON ({e}): {stdout:?}"));
    assert_eq!(v["kind"], "error");
    assert_eq!(
        v["error_kind"], "not_found",
        "the DTO must carry the typed kind, not just prose: {v}",
    );
    assert_eq!(out.status.code(), Some(2));

    shutdown(&home);
    std::fs::remove_dir_all(&home).ok();
}

/// Stand a fake daemon on `home`'s control socket, replying `reply` to every
/// request, and run `lait <args>` against it. Returns (stderr, exit code).
#[cfg(unix)]
fn against_fake_daemon(
    tag: &str,
    reply: &'static [u8],
    args: &[&str],
) -> (String, Option<i32>, Duration) {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;

    let home = tmp_home(tag);
    init(&home);
    shutdown(&home);
    std::thread::sleep(Duration::from_millis(500));

    let sock = lait::config::socket_path(&home);
    std::fs::remove_file(&sock).ok();
    let listener = UnixListener::bind(&sock).expect("bind fake daemon");
    let fake = std::thread::spawn(move || {
        for stream in listener.incoming().take(8) {
            let Ok(mut s) = stream else { continue };
            let mut line = String::new();
            BufReader::new(s.try_clone().unwrap())
                .read_line(&mut line)
                .ok();
            s.write_all(reply).ok();
            s.write_all(b"\n").ok();
        }
    });

    let started = Instant::now();
    let out = lait(&home, args);
    let elapsed = started.elapsed();

    drop(fake);
    std::fs::remove_file(&sock).ok();
    std::fs::remove_dir_all(&home).ok();
    (
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code(),
        elapsed,
    )
}

/// A daemon this build can't talk to must be reported as *present and foreign* —
/// promptly — not as absent.
#[cfg(unix)]
#[test]
fn a_foreign_daemon_is_named_not_timed_out() {
    // A pre-handshake daemon (v0.4.8): it has no `hello`, so serde rejects the
    // request as an unknown variant. That rejection is the identification.
    let (stderr, code, elapsed) = against_fake_daemon(
        "foreign",
        br#"{"kind":"error","message":"bad request: unknown variant `hello`","error_kind":"error"}"#,
        &["status"],
    );

    // The old path spawned a doomed daemon and polled for a full 20s first.
    assert!(
        elapsed < Duration::from_secs(10),
        "a foreign daemon must be diagnosed promptly, took {elapsed:?}",
    );
    assert_ne!(code, Some(0), "must not report success; stderr: {stderr}");
    assert!(
        stderr.contains("already running"),
        "must say a daemon is there, not imply none is; got: {stderr}",
    );
    assert!(
        !stderr.contains("did not come online"),
        "must not blame a timeout for a daemon that answered instantly; got: {stderr}",
    );
}

/// The asymmetry, end to end: a daemon *ahead* of this build must never be
/// stopped — not even under `--yes`, which is exactly when a blunt "clean it up"
/// would fire. Replacing it downgrades the node, and a store already written at a
/// newer `SCHEMA_VERSION` would then refuse to open at all.
#[cfg(unix)]
#[test]
fn a_newer_daemon_is_never_replaced_even_with_yes() {
    let (stderr, code, _) = against_fake_daemon(
        "newer",
        br#"{"kind":"hello","protocol_version":9000}"#,
        &["--yes", "status"],
    );

    assert_ne!(code, Some(0), "must not proceed; stderr: {stderr}");
    assert!(
        stderr.contains("lait update"),
        "the way out of being behind is to upgrade, not to kill it; got: {stderr}",
    );
    assert!(
        !stderr.contains("stopped it"),
        "must never stop a daemon newer than this build; got: {stderr}",
    );
}
