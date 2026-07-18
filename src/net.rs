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
//! **Reachability.** Every peer dial in `node.rs` is a bare `EndpointId`, which
//! iroh resolves through the endpoint's address lookups (the address-free design
//! in `crate::proto`). `Public` gets that resolution from n0 discovery. `Local`
//! has NO discovery service — instead lait registers `{id, relay}` for each peer
//! it learns into a [`PeerBook`] (an in-process `MemoryLookup`), because lait
//! already knows its relay and can build the address directly. Nothing is
//! discovered over the wire and nothing is faked — the exact pattern iroh-gossip
//! uses for bootstrap. `Isolated` still has no wired connectivity (it would need
//! direct addresses to travel in tickets — a separate change).

use anyhow::{Context, Result};
use iroh::{
    address_lookup::MemoryLookup, endpoint::presets, Endpoint, EndpointAddr, EndpointId, RelayMap,
    RelayMode, RelayUrl, SecretKey,
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

/// The reachability address for `id` under a relay policy: `{id, relay}`. lait
/// KNOWS its relay (it configured it), so it builds this directly — no discovery
/// service is consulted, and nothing is faked. This is the one iroh-typed
/// address construction, shared by the daemon (via [`PeerBook`]) and the tests.
pub fn relay_addr(relay: &RelayUrl, id: EndpointId) -> EndpointAddr {
    EndpointAddr::new(id).with_relay_url(relay.clone())
}

/// How the daemon teaches its endpoint to reach peers under lait's policy.
///
/// Every dial in `node.rs` is a bare `EndpointId`; iroh resolves it through the
/// endpoint's address lookups. Under `Public` that resolution is n0 discovery.
/// Under `Local` there is no discovery — so lait registers `{id, relay}` for each
/// peer it learns into an in-process [`MemoryLookup`] the endpoint queries. No
/// discovery service, plaintext or otherwise; lait supplies the address it
/// already knows. (This is the pattern iroh-gossip itself uses for bootstrap.)
#[derive(Clone)]
pub struct PeerBook {
    lookup: MemoryLookup,
    relay: Option<RelayUrl>,
}

impl PeerBook {
    /// Teach the endpoint how to reach `id`. Under `Local` that is `{id, relay}`;
    /// under `Public` n0 discovery already resolves ids (no-op); `Isolated` has
    /// no relay and would need carried direct addresses (no-op for now).
    pub fn learn(&self, id: EndpointId) {
        if let Some(relay) = &self.relay {
            self.lookup.add_endpoint_info(relay_addr(relay, id));
        }
    }
}

/// Build the iroh endpoint that fulfils lait's [`Network`], plus the [`PeerBook`]
/// the daemon populates so bare-id dials resolve. This is the sole contractor
/// boundary: the only function that names iroh's relay/discovery types. A future
/// transport swap rewrites this and nothing above it.
pub async fn build_endpoint(secret_key: &SecretKey, net: &Network) -> Result<(Endpoint, PeerBook)> {
    // One in-process address book, queried by the endpoint and populated by the
    // daemon. Under Public it is a harmless extra cache; under Local it is the
    // resolution mechanism.
    let lookup = MemoryLookup::new();
    let mut relay = None;
    let builder = match net {
        // Public: n0's relays + discovery, plus the in-process address cache the
        // daemon has always used.
        Network::Public => Endpoint::builder(presets::N0).address_lookup(lookup.clone()),
        // Isolated: no relay, no discovery. Direct reach only.
        Network::Isolated => Endpoint::builder(presets::Minimal)
            .relay_mode(RelayMode::Disabled)
            .address_lookup(lookup.clone()),
        // Local: lait's own single relay. Reachability is relay-based — the
        // daemon registers `{id, relay}` per peer into `lookup`, so a bare id
        // resolves with no discovery, which is what makes it hermetic.
        Network::Local(l) => {
            let url: RelayUrl = l.relay.parse().context("LAIT_RELAY is not a valid URL")?;
            relay = Some(url.clone());
            Endpoint::builder(presets::Minimal)
                .relay_mode(RelayMode::Custom(RelayMap::from(url)))
                .address_lookup(lookup.clone())
        }
    };
    let endpoint = builder
        .secret_key(secret_key.clone())
        .bind()
        .await
        .context("bind iroh endpoint")?;
    Ok((endpoint, PeerBook { lookup, relay }))
}
