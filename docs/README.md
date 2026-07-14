# lait docs

Design and operator documentation for **lait**, a local-first, peer-to-peer issue
tracker (Loro CRDTs · git-backed store · iroh P2P). For the project overview and
quickstart, see the top-level [`README.md`](../README.md); for per-version detail, see
[`CHANGELOG.md`](../CHANGELOG.md).

**Current state (v0.4.8):** P0–P3 complete and verified multi-node; P4 (release
engineering) shipped, with security review and receipt/tier hardening the main deferrals.

## The three design legs

The architecture is documented as three complementary docs that cross-reference each
other by a short section notation — `A§5` means ARCHITECTURE §5, `S§7` SCHEMA §7, `U§4`
UI §4. They are the design of record, kept in sync with the shipped code.

| Doc | Notation | Covers |
|---|---|---|
| [`ARCHITECTURE.md`](./ARCHITECTURE.md) | `A§` | The system: layered design, the git/iroh/Loro split, sync protocol, seed role, E2EE model, decision log. |
| [`SCHEMA.md`](./SCHEMA.md) | `S§` | The data shapes across the three layers (CRDT storage / control protocol / wire) and **what authority each field carries**. |
| [`UI.md`](./UI.md) | `U§` | The three drive surfaces — CLI, TUI, MCP — and the one imperative façade they share over the CRDT. |

## Focused designs

| Doc | Status | Covers |
|---|---|---|
| [`GUIDED-JOIN.md`](./GUIDED-JOIN.md) | shipped (v0.4.7) | The first-invite verifier (`lait doctor`) and the directory-trap fix. |
| [`HARDENING.md`](./HARDENING.md) | proposed (deferred) | Agent-messaging delivery/ack receipts and urgency tiers ("notify anyway"). Not yet built. |

## Operator docs

| Doc | Covers |
|---|---|
| [`INSTALL.md`](./INSTALL.md) | Every install channel (shell/PowerShell installers, Homebrew, Scoop, winget, Cargo, Docker seed), download verification, completions, and the man page. |
| [`ROADMAP.md`](./ROADMAP.md) | The P0→P4 execution plan, the Definition of Done, the CI gate, and per-phase status. |

## Reading order

New to the project: top-level [`README.md`](../README.md) → [`ARCHITECTURE.md`](./ARCHITECTURE.md)
→ [`SCHEMA.md`](./SCHEMA.md) → [`UI.md`](./UI.md). Installing or operating a node:
[`INSTALL.md`](./INSTALL.md). Tracking what's done: [`ROADMAP.md`](./ROADMAP.md) and
[`CHANGELOG.md`](../CHANGELOG.md).
