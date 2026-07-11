# Changelog

## v0.3.0 — the P2P, E2EE issue tracker (release candidate)

groupchat becomes a working **local-first, peer-to-peer, end-to-end-encrypted
issue tracker** — a decentralized, rapid-feedback alternative to Linear that runs
as a native Rust node, built on [iroh](https://www.iroh.computer/) (P2P QUIC) and
[Loro](https://loro.dev/) CRDTs over a git-backed durable store. Verified
multi-node over real iroh on Linux, macOS, and Windows.

### Highlights

- **A fast, standalone tracker (P0).** Create / edit / move / assign / label /
  comment / close issues from a CLI, a full-screen [ratatui](https://ratatui.rs)
  TUI, or an MCP agent — all driving one daemon that owns the Loro documents.
  Boards and lists render from a catalog cache (no per-issue loads); issues carry
  a short git-style `iss_` handle plus a friendly `ENG-142` alias. The TUI stays
  live off a doorbell event stream and echoes edits optimistically.
- **Live P2P sync (P1).** Catalog-first sync over a custom iroh ALPN: two nodes
  converge in ~2s with no central server. A portable **seed** role — any headless
  node advertised in a ticket — backfills a cold client from nothing but the
  ticket. Three-state presence (online / away / offline).
- **End-to-end encryption + membership (P3).** Workspace data is E2EE, gated by a
  **signed ed25519 ACL op-graph** (add / remove / roles, deterministic replay,
  remove-wins). The workspace key is distributed via X25519 sealed boxes and
  **rotated on removal** (lazy revocation); a non-member — or a removed member —
  sees only ciphertext. `members add/remove/rotate-key/ls` on the CLI, MCP, and a
  TUI members view. Pure-Rust crypto (RustCrypto/dalek) — no C toolchain, no
  `aws-lc`.
- **Agent-native (MCP).** The full tracker surface is exposed as MCP tools that
  return the same versioned DTO the CLI `--json` emits; a build-gate parity test
  keeps the human and agent surfaces in lock-step.

### Cross-platform & release

- Builds and runs on **Linux, macOS, and Windows**; the hardened CI gate (build +
  test with `-D warnings`, fmt, clippy, doctests, MSRV 1.91, `cargo-deny`,
  portability guard, DTO/MCP parity, a per-OS end-to-end smoke, and a release
  dry-run) is green on all three.
- Prebuilt binaries for macOS (arm64 + x86), Linux (arm64 + x86), and **Windows
  (x64)**, with shell + PowerShell installers, per-target self-updater, and
  SHA-256 checksums.

Install (once released):

```sh
# macOS / Linux
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/Nixie-Tech-LLC/groupchat/releases/download/v0.3.0/groupchat-installer.sh | sh
# Windows (PowerShell)
powershell -ExecutionPolicy Bypass -c "irm https://github.com/Nixie-Tech-LLC/groupchat/releases/download/v0.3.0/groupchat-installer.ps1 | iex"
```

Upgrade in place with `groupchat-update`.

### Known limitations (accepted / deferred)

- The E2EE layer implements a proven *design* by hand and is **research-grade**:
  unaudited, and it needs independent review before carrying truly sensitive data.
- Lazy revocation only (no clawback of already-synced data); metadata (sizes,
  timing) is visible to a relay; all members of a workspace read all its issues.
- The blind-relay **ciphertext-chunk sedimentree** compaction (P2) is designed but
  its GC is deferred — encrypted sync already makes the seed a blind relay.
- Deferred: RIBLT scale escape-hatch, account-aggregates-devices identity, and a
  CGKA (BeeKEM) key-agreement upgrade over the current sealed-box distribution.

Foundation preserved from the earlier chat-oriented releases: the iroh endpoint +
ed25519 identity, signed-gossip room, presence, daemon + cross-platform control
channel, CLI, and MCP plumbing.
