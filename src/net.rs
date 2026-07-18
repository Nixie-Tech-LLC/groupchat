//! **Network policy** — lait's own requirement for the transport environment,
//! with iroh as the contractor that fulfils it.
//!
//! lait states *where* it operates — the public relay mesh, a named local relay,
//! or isolated — and this module is the **single place** iroh's relay/discovery
//! vocabulary (`RelayMode`, `presets`, `address_lookup`) is spoken. Above
//! [`build_endpoint`] the rest of the daemon knows only [`Network`]; there is no
//! "iroh default" anymore, only lait's `Public` policy that iroh executes.
//!
//! The point is ownership of *behaviour*: lait chooses its transport
//! environment instead of inheriting n0's. This seam establishes that ownership
//! and the single contractor boundary.
//!
//! **Status.** Only [`Network::Public`] is wired end-to-end for the daemon. Every
//! peer dial in `node.rs` is a bare `EndpointId` resolved via discovery (the
//! address-free design in `crate::proto`); `Public` gets that from n0. `Local`
//! and `Isolated` configure the endpoint but supply no discovery and attach no
//! address, so a bare id resolves to nothing — the daemon will start but not
//! converge until the dial paths attach `{id, relay}` addresses (via a
//! `MemoryLookup` the daemon populates, or self-hosted pkarr/DNS). The daemon
//! warns loudly on any non-`Public` policy for exactly this reason. Wiring that
//! is the next step; this commit lands the seam, not a working `Local` daemon.

use anyhow::{Context, Result};
use iroh::{
    address_lookup::MemoryLookup, endpoint::presets, Endpoint, RelayMap, RelayMode, RelayUrl,
    SecretKey,
};

/// lait's requirement for the transport environment. iroh executes it.
#[derive(Debug, Clone)]
pub enum Network {
    /// The public relay mesh + public discovery (n0). The default — unchanged
    /// behaviour, now stated rather than inherited. The **only** policy whose
    /// daemon peer-connectivity is wired today (n0 discovery resolves the bare
    /// `EndpointId`s that every dial in `node.rs` uses).
    Public,
    /// A single relay lait supplies (self-hosted, or a test harness's in-process
    /// relay). The endpoint is configured for it, but **daemon connectivity is
    /// not yet wired**: lait dials peers by bare `EndpointId` and relies on
    /// discovery to resolve them (`crate::proto` §"address-free"), and `Local`
    /// provides no discovery — so a peer id resolves to nothing until the dial
    /// paths attach `{id, relay}` addresses (the follow-up). See the crate docs.
    Local(LocalNet),
    /// No relay, no discovery — direct reach only. Same wiring gap as `Local`,
    /// plus it would need addresses to travel in tickets. Endpoint construction
    /// is defined; daemon connectivity is not.
    Isolated,
}

/// A single relay lait supplies. Once the dial paths attach `{id, relay}`
/// addresses (see the crate-level status note), reachability is relay-based — a
/// peer is `{its id, this relay}`, needing no public discovery, which is what
/// makes it hermetic. lait names the relay in a plain URL; iroh is the contractor.
#[derive(Debug, Clone)]
pub struct LocalNet {
    /// The relay URL peers rendezvous through (`https://…` / `http://…`). A
    /// self-hosted relay presenting a valid certificate. (Self-signed / dev
    /// relays require skipping cert verification, which iroh gates to test
    /// builds — so that path lives in the test harness, not here.)
    pub relay: String,
}

impl Network {
    /// Resolve the requirement from the environment, defaulting to [`Public`] so
    /// existing deployments are unchanged. `LAIT_NETWORK` = `public` (default) |
    /// `local` | `isolated` (trimmed, case-insensitive); `local` reads
    /// `LAIT_RELAY`. An unknown value is an error, never a silent default.
    ///
    /// [`Public`]: Network::Public
    pub fn from_env() -> Result<Self> {
        let raw = std::env::var("LAIT_NETWORK").unwrap_or_default();
        match raw.trim().to_ascii_lowercase().as_str() {
            "" | "public" => Ok(Network::Public),
            "isolated" => Ok(Network::Isolated),
            "local" => Ok(Network::Local(LocalNet {
                relay: env_req("LAIT_RELAY")?,
            })),
            other => {
                anyhow::bail!("unknown LAIT_NETWORK '{other}' (expected public|local|isolated)")
            }
        }
    }

    /// Whether this policy provides a relay — so waiting on `endpoint.online()`
    /// (which blocks on a home relay) is sound. Isolated has none.
    pub fn uses_relay(&self) -> bool {
        !matches!(self, Network::Isolated)
    }
}

fn env_req(key: &str) -> Result<String> {
    std::env::var(key).with_context(|| format!("LAIT_NETWORK=local requires {key}"))
}

/// Build the iroh endpoint that fulfils lait's [`Network`]. This is the sole
/// contractor boundary: the only function in the codebase that names iroh's
/// relay and discovery types. A future transport swap rewrites this and nothing
/// above it.
pub async fn build_endpoint(secret_key: &SecretKey, net: &Network) -> Result<Endpoint> {
    let builder = match net {
        // Public: n0's relays + discovery, plus the in-process address cache the
        // daemon has always used.
        Network::Public => Endpoint::builder(presets::N0).address_lookup(MemoryLookup::new()),
        // Isolated: no relay, no discovery. Direct reach only.
        Network::Isolated => Endpoint::builder(presets::Minimal)
            .relay_mode(RelayMode::Disabled)
            .clear_address_lookup(),
        // Local: lait's own single relay. Reachability is relay-based (a peer is
        // `{its id, this relay}`), so no public discovery is involved — that is
        // what makes it hermetic and offline-capable.
        Network::Local(l) => {
            let relay: RelayUrl = l.relay.parse().context("LAIT_RELAY is not a valid URL")?;
            Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Custom(RelayMap::from(relay)))
        }
    };
    builder
        .secret_key(secret_key.clone())
        .bind()
        .await
        .context("bind iroh endpoint")
}
