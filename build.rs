//! Build-time version stamping. Exposes `LAIT_VERSION_LONG` — the string `lait
//! --version` prints — as a compile-time env the CLI reads via `env!`.
//!
//! Stable/release builds print a clean semver (`0.4.5`). A **dev-channel** build
//! (the `Dev Release` workflow, or any build that sets `LAIT_BUILD_SHA`) appends a
//! `-dev+<sha> (<date>)` suffix so a nightly binary is unmistakable from a tagged
//! release. We deliberately read **only** explicit env vars — never shell out to
//! git — so a normal `cargo install` / cargo-dist release stays clean and
//! reproducible, and only the dev workflow opts into the suffix.

use std::env;

fn main() {
    // Re-run only when the stamping inputs change (not on every source edit).
    println!("cargo:rerun-if-env-changed=LAIT_BUILD_SHA");
    println!("cargo:rerun-if-env-changed=LAIT_BUILD_DATE");

    let base = env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let sha = env::var("LAIT_BUILD_SHA").unwrap_or_default();
    let date = env::var("LAIT_BUILD_DATE").unwrap_or_default();

    let long = if sha.is_empty() {
        base
    } else if date.is_empty() {
        format!("{base}-dev+{sha}")
    } else {
        format!("{base}-dev+{sha} ({date})")
    };

    println!("cargo:rustc-env=LAIT_VERSION_LONG={long}");
}
