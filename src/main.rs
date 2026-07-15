//! lait binary shim. All logic lives in the `lait` library crate
//! (see `lib.rs` and `app::run`) so tests, doctests, and the MCP/DTO parity
//! check exercise the same code paths the binary runs.
//!
//! Deliberately returns [`ExitCode`], not `Result`: returning `Result` hands
//! every error to anyhow's `Termination` impl, which Debug-prints the `Caused
//! by:` chain (leaking postcard/base32 internals), ignores `--json`, and exits
//! `1` regardless of what went wrong. `app::run` owns reporting instead, because
//! only it knows the output mode the error has to be rendered in.

use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    lait::app::run().await
}
