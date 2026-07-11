//! lait binary shim. All logic lives in the `lait` library crate
//! (see `lib.rs` and `app::run`) so tests, doctests, and the MCP/DTO parity
//! check exercise the same code paths the binary runs.

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    lait::app::run().await
}
