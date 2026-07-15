# UI — lait: CLI & TUI

> **Status:** implemented (v0.4.8); this is the design of record, kept in sync with
> the shipped surfaces. The third design leg, companion to
> [`ARCHITECTURE.md`](./ARCHITECTURE.md) (refs `A§`) and [`SCHEMA.md`](./SCHEMA.md)
> (refs `S§`). Covers the two human surfaces of the tracker — the **CLI** and the
> **TUI** — plus the agent surface (MCP) they share a contract with. The full
> **P0-complete** surface (single node, git-backed) is built, and the P1/P3 surfaces
> it slotted (§8 — live sync/presence, membership) have since landed; P4 polish is the
> remaining work. Decisions are flagged **[DECISION]** with the shipped default in bold,
> same as S§.

## 1. Scope & the one-façade rule

There are exactly **three ways to drive a node**, and they are **the same imperative
façade over the CRDT** (S§7, Layer B) — never three parallel implementations:

| Surface | What it is | Who uses it | Talks to the daemon via |
|---|---|---|---|
| **CLI** | one-shot verbs, scriptable, `--json` | humans in a shell, scripts, agents | control socket, request→response |
| **TUI** | full-screen interactive board | humans at a terminal | control socket, request→response **+ a live event stream** |
| **MCP** | tool-call surface (A§12) | agents | control socket (same requests, JSON-shaped) |

**The rule (extends S§1):** all three are **thin clients of the daemon**; the daemon is
the *only* thing that owns a Loro doc. A surface never embeds the node, never touches
`.loro` files, never re-implements merge. It **sends a `Request`, gets a `Response`
snapshot, and — for live surfaces — consumes the `IssueEvent` stream.** This is the whole
reason Layer B is a hand-maintained projection (S§1): the three surfaces are its three
consumers, and the TUI is not privileged over the CLI.

**Consequence for the TUI (the load-bearing decision, confirmed).** The TUI is a
**daemon client over IPC**, *not* an in-process embedding of the node. It renders from
`Response` snapshots and patches itself from the `IssueEvent` stream; edits are `Request`s.
"Optimistic local ops + instant render" (A§9) is achieved by **client-side optimistic echo
over a local IPC hop** (§4.3), not by the TUI holding its own Loro replica. One node
process, one source of truth, one Layer-B contract to keep stable — the TUI inherits the
same refactor-freedom the contract buys the CLI and MCP.

**Design tenets (Linear-grade devex, the plan's foundation — A§1):**
1. **Keyboard-first, mouse-optional.** Every action has a key; nothing *requires* a mouse.
2. **One `Request` = one Loro commit = one activity row** (S§7.1). The command surface
   *defines* the activity-feed granularity, so verbs are drawn at commit boundaries.
3. **Instant feel.** Reads render from the Catalog cache (no issue-doc loads, A§9); writes
   echo optimistically and self-heal on the authoritative event.
4. **Same nouns everywhere.** A `Ref` means the same thing in the CLI, the TUI command
   palette, and an MCP tool argument (§3).

> `src/control.rs` now carries the **tracker** Layer B specified here (the S§7 enum):
> the issue verbs, the membership/ACL verbs, `Subscribe`, and `Diagnose`. The chat-era
> transport/presence verbs (`Status/Invite/Join/Connect/Log/Who/Stop`) survive
> alongside as the P1 networking surface (§8).

## 2. CLI command surface

Invocation: `lait [--home DIR] [-w SEL] [--json] [--no-color] [<command> [args]]`.
**Bare `lait` is the focus view**: your unread inbox summary + your open issues — the
most valuable keystroke answers "what's addressed to me / what am I on", never help.
`--home` selects a self-contained node (`$LAIT_HOME`); `-w/--space` (alias
`--workspace`) selects a **space** from any directory by name, `ws_` id (or unique
prefix), or path — resolved through the registry to a store path (precedence: `--home`
> `-w` > cwd discovery); `--json` switches every command to the versioned DTO (§2.3);
the daemon is auto-spawned on first use (existing `ensure_daemon`). Commands never
create a store implicitly: in a directory with no space they error with guidance
(`init`/`join`/`-w`).

> **Vocabulary:** the user-facing noun is **space**; the architecture documents keep
> the internal term *workspace* (`WorkspaceId`, the Catalog's `workspaceId`, the
> `workspace` doctor-gate id). Same thing, two altitudes.

### 2.1 Command table

Verbs act on **issues**; plural nouns manage **registries**. Each maps to exactly one S§7
`Request`.

| Command | `Request` (S§7) | Description |
|---|---|---|
| `init [--name N] [--nick N]` | — | **Found a workspace here** (`cwd/.lait`): mints the genesis, names it (default: the directory), seeds a first project so `new` works immediately. Errors inside an existing workspace. |
| `new <title> [-p PROJ] [-a USER…] [-P PRIO] [-l LABEL…] [-b BODY] [--start]` | `IssueNew` | Create an issue; echoes the resolved handle (`Response::Ref`). `-p` optional — the S§7.6 chain (branch key → `project.default` → sole project). Unknown `-l` labels are **created on first use** (vocabulary, not ceremony). `--start` chains straight into the work loop. |
| `start [ref] [--no-branch]` | `IssueStart` | **Claim + activate + branch** in one intent: assign yourself, move to the first Active-category status (one commit = one activity row), then create+checkout `key-n-slug`. Ref inferred from the branch when omitted; branch step is best-effort, skipped outside git. Returns the fresh `Response::Issue` (the one writes-echo-Ref deviation — the CLI needs the title for the slug). |
| `done [ref]` | `IssueDone` | Finish: first Done-category status (assignee kept, S§5.7 board removal). Ref inferred from the branch — the loop closes with no ref typed. |
| `stop [ref]` | `IssueStop` | Put it down gracefully: first Backlog-category status, unassign yourself. |
| `inbox [--clear]` | `Inbox` | The **durable, addressed-to-you** inbox (S§8.1): remote assignments, comments on your work, `@nick` mentions, status moves — newest-first with an unread watermark. Sits BESIDE `activity` (the workspace firehose): two different questions, two commands. |
| `ls [-p PROJ] [--mine] [--status S] [--label L] [--all]` | `List` | List rows from the Catalog cache only (no issue-doc loads). `-p` is a pure filter (never defaulted); `--all` includes done/tombstoned. |
| `board [PROJ]` | `Board` | Render the project's columns (workflow states × ordered rows). Positional optional — the S§7.6 chain. |
| `show <ref>` | `IssueView` | Full issue — **lazy-loads the issue doc**. Body, comments, activity. |
| `edit <ref> [--title T] [--status S] [--priority P]` | `IssueEdit` | Patch the LWW fields. Multiple flags = **one** commit = one activity row (S§7.1). |
| `move <ref> [-p PROJ] [--top\|--bottom\|--before R\|--after R]` | `IssueMove` | Set project (truth) and/or board position (order). `-p` explicit only — membership is never inferred. |
| `assign <ref> <userref…> [--remove]` | `Assign` | Add/remove assignees (present-key set, S§5.2). |
| `label <ref> [+LABEL…] [-LABEL…]` | `Label` | Add (`+`) / remove (`-`) labels on an issue. |
| `comment [ref] [BODY]` | `Comment` | Append a comment (immutable body, S§5.3). One arg on a KEY-n branch = the body, ref inferred (the branch-native loop); no BODY → read stdin. |
| `delete <ref>` | `IssueDelete` | Tombstone an issue (S§5.6); it stays in `docs` for history/backfill, `ls`/`board` hide it. |
| `history <ref>` | `History` | The issue's derived activity/time-travel feed (free from Loro op history, A§5). |
| `projects [add KEY [NAME] \| ls]` | `ProjectNew`/`ProjectList` | Manage the project registry (`Catalog.projects`). Key-first, name optional (defaults to the title-cased key); `new` kept as an alias of the same shape. |
| `labels [new <name> --color C \| ls]` | `LabelNew`/`LabelList` | Manage the label registry (`Catalog.labels`). |
| `members [add\|remove\|requests\|approve\|name\|rotate-key\|ls]` | `MemberAdd`/`MemberRemove`/… | Manage E2EE membership (the signed ACL, S§6): `add` seals the key, `remove` rotates it, `approve` admits a pending joiner, `name` sets a local alias (§8, P3). |
| `activity [--since N]` | `Activity` | Workspace-wide recent transitions (ex-`log`; ring-buffer `seq`). |
| `watch [--since N] [--exec CMD] [--notify]` | `Subscribe`-stream | Follow forever; run a hook / desktop-notify per event. The scripting primitive. |
| `tui` | — | Launch the full-screen board (§4). |
| `doctor` (alias `verify`) | `Diagnose` | Guided-join verifier: names the one onboarding gate that's blocking ([`GUIDED-JOIN.md`](./GUIDED-JOIN.md)). Auto-tails `join`. |
| `spaces [ls\|forget <sel>\|prune]` (alias `workspaces`) | — | Every space on this machine (founded + joined): name, id, origin, live status (`up`/`idle`/`missing`), project keys, path. `forget` deregisters (never touches disk); `prune` drops missing entries. |
| `config [get\|set\|unset\|ls]` | — | Layered local settings, git-style: global `config.json` + per-store `config.json` (store wins). Keys: `user.nick` (daemon-read → live `ConfigReload` on set), `project.default`; `workspace.*` reserved for future synced settings. Daemon-free. |
| `profiles` (alias `agents`) · `resume <name>` | — | List / switch named profiles (each a separate identity + store). |
| `status` · `shutdown` · `id` | `Status`/`Stop`/`Id` | Node/space status; stop the daemon (`stop` the word belongs to the work loop); print the endpoint id. |
| `invite` · `join [--dir D]` (alias `connect`) · `who` · `remote` (alias `seed`) | (P1 transport, §8/A§8) | The networking surface: invite/join a workspace, list peers, pin a seed. `join` **creates** the joiner's store (cwd or `--dir`) from the ticket before the daemon runs; joining from a directory bound to a different workspace is a hard exit-2 error. |

### 2.2 Notable behaviors

- **Writes echo the resolved handle.** `new`/`edit`/`move`/… return `Response::Ref{reff}`
  so a script can capture the canonical handle (`iss_…` short prefix, §3) it just touched:
  `id=$(lait new "fix login" -p ENG --json | jq -r .reff)`.
- **Branch-inferred refs.** On a git branch whose name embeds a `KEY-n` (e.g.
  `eng-142-fix-login`), the `<ref>` is **optional** for `show`/`edit`/`move`/`history`/
  `delete` — lait infers `ENG-142` from the branch, mirroring the git-companion workflow.
- **Branch-inferred project.** The same branch also yields the project KEY (`ENG`),
  shipped to the daemon as a separate `project_hint` for `new`/`board` (S§7.6): used only
  if it resolves to a real project, so a branch like `wip-2` never breaks anything, and an
  explicit `-p` miss still errors loudly.
- **No compare-and-swap (S§7.2).** There is no `--if-status open` flag and there never will
  be one; a `Response` is a snapshot with no cursor back into the doc, edits merge, and
  "close only if still open" is inexpressible. Stated here so nobody adds optimistic
  concurrency to the CLI later.
- **`ls`/`board` never open issue docs.** They render the `DocMeta` cache (S§4). A row for a
  doc whose issue body hasn't synced yet is **provisional** and marked so (§3.3) — expected,
  not an error.
- **Done issues.** `ls`/`board` hide done + tombstoned by default (S§5.6–5.7). `--all`
  includes them; the **Done** column renders via the append rule (S§5.5) ordered by
  wall-clock desc, since done issues leave `boards[proj]`.

### 2.3 The `--json` contract

`--json` prints the **stable, versioned `Response` DTO** (S§7.3) — the *same* shape MCP
tools return. This is a **public contract**: agents and scripts consume it, so it is
hand-maintained and MUST NOT track the Loro layout automatically (S§1, S§7.3). Every DTO
carries the `schemaVersion` gate (S§9) so a reader can detect drift.

- Read commands emit their projection (`Row[]`, `BoardView`, `IssueView`, `Event[]`).
- Write commands emit `Response::Ref` or `Response::Ok`.
- Errors emit `Response::Error{message}` on stdout under `--json` (exit non-zero), never a
  bare stderr string, so a pipeline can branch on it.

**Exit codes:** `0` ok · `1` usage/parse error · `2` ref not found / ambiguous (§3.2) ·
`3` daemon unreachable. Machines branch on the code; humans read the message.

## 3. Refs & addressing — one grammar, resolved daemon-side

All three surfaces accept the **same** ref grammar (S§2, S§7). Resolution happens in the
**daemon**, never the client, so the grammar can grow without touching a surface.

### 3.1 The grammar

- **`<ref>`** (an issue) accepts, in priority order: a **short `DocId` prefix**
  (`iss_3f9`, git-style — the *canonical*, collision-free handle, S§5.4); a **`KEY-n`
  alias** (`ENG-142`, advisory, may disambiguate); or — only where a project is expected
  (`ls`/`board`) — a **project key** (`ENG`).
- **`<userref>`** (a member) accepts: **`@me`** (this node's `UserId`); a **local
  alias** (a petname *you* assigned to a key, stored locally, never synced); a **key
  id-prefix** (≥4 hex); or a full **ed25519 key** (S§2 — a member *is* a key). A
  self-asserted wire nick is **not** accepted: only a locally-trusted alias resolves
  to a key, so an unauthenticated name can never stand in for an identity.

### 3.2 Ambiguity is a first-class outcome

Because `KEY-n` may collide (S§5.4) and a short prefix may be too short, resolution can
return **zero or many** matches. The daemon answers:
- **exactly one** → resolved; proceed.
- **zero** → `Error{ "no issue matches 'ENG-9x'" }`, exit `2`.
- **many** → `Error` listing the candidates with the shortest disambiguating prefix
  (`iss_3f9a…`, `iss_3f9b…`); the caller re-issues with more characters. The CLI prints the
  candidate list; the TUI shows a picker (§5.6).

The **canonical** handle in all output is the short `DocId` prefix; `KEY-n` is shown as a
friendly alias beside it, never as the sole identifier (S§5.4).

### 3.3 Provisional rows

A ref can resolve to a doc that exists in `Catalog.docs` but whose **issue body hasn't
arrived** (a peer synced the Catalog first, A§9). `show` on such a ref returns the
provisional `DocMeta` projection flagged `provisional: true`; the TUI dims it (§4.4). When
the issue doc arrives, the row self-heals (S§3.1). This only occurs post-P1; at P0 every doc
is local, but the flag is designed now so the surfaces don't need reshaping later.

## 4. TUI architecture & reactivity

`lait tui` is a [ratatui](https://ratatui.rs) full-screen client. **[DECISION] ratatui**
— it is the mature, portable (crossterm on all three OSes, matching the cross-platform bar
in A§ decision log) Rust TUI substrate; no real alternative. It renders from Layer-B
snapshots and stays live off the event stream.

The central design fact: **the event stream is doorbells, not deltas.** An event never
carries the new state; it *rings* — "scope S is dirty" — and the client re-reads the
authoritative projection for S. The daemon owns every Loro doc and every merge; the TUI only
ever holds a *prediction* (its optimistic overlay) and a *cache of the daemon's cache*. This
is what makes reconciliation correlation-free (§4.3): there is no op-id to match, no payload
to trust, no partial patch to mis-apply — just "a doorbell rang → re-read → repaint."

### 4.1 Process & connection model

On launch the TUI runs `ensure_daemon` (identical to the CLI), then opens **two** control
connections over the one socket:

```
        ┌─ command channel ──> Request  ──> Response       (issue ops, snapshot loads)
 TUI ───┤
        └─ subscribe channel <── Doorbell stream …          (live dirty-notices, §4.2)
```

- **Command channel:** ordinary request→response (the existing `control::request` path),
  reused for every edit and every snapshot re-read.
- **Subscribe channel:** one long-lived connection carrying the live doorbell stream. This is
  the one Layer-B addition the TUI needs (S§7):

  > **`Subscribe { since: u64 }`** — turns the one-shot handler into a **streaming
  > mode**: the daemon reads the request, then instead of returning after one response, parks
  > on the doorbell `Notify` and writes newline-delimited **`Doorbell` frames** until the
  > client hangs up or the daemon stops. **[DECISION] streaming Subscribe is the one live
  > channel**: it pushes with no per-round request overhead, and every plane rings it — the
  > tracker dirty-set, `activity_advanced`, and `presence_advanced` (the presence/join plane
  > CLI `watch` follows). The re-polling `Wait` verb it superseded is gone: it duplicated the
  > wake path with a worse restart story (no epoch, so a stale cursor went silently deaf).

**Reconnect, restart, and gaps all collapse to one path — `Reset`.** `seq` is per-daemon
*session*, not durable (S§2): a daemon restart (crash, or the routine idle-shutdown) resets
it to 0, and the ring buffer holds only the last ~1000 doorbell *batches*, so a client can
fall off the back. Rather than special-case each, the stream emits a **`Reset` doorbell**
meaning *"your position is invalid — rebaseline from a fresh snapshot."* The TUI handles it
identically to first-connect: pull `Board`/`List` snapshots, adopt them wholesale, resume
`Subscribe` from the snapshot's `last`. The daemon rings `Reset` (a) as the **first frame** of
every `Subscribe`, and (b) whenever a client's `since` is older than the oldest retained batch
or newer than current `seq`. A small **per-boot epoch nonce** on every response lets a client
detect a restart even without a socket drop; a changed epoch ⇒ treat as `Reset`. Because
doorbells are idempotent dirty-flags, rebaselining is always safe and `seq` never needs
persisting.

### 4.2 The doorbell stream

A doorbell is a **batched, project-keyed dirty-set** — never a value:

```
Doorbell { epoch, seq,
           dirty_by_project : Map<ProjectId, [DocId…]>,   // issue-row plane
           dirty_catalog    : [projects | labels | acl | workflow | boards(proj)],  // structure plane
           activity_advanced: bool,                        // "new feed rows exist"
           reset            : bool }                        // rebaseline, ignore the rest
```

Two authority planes ring through the one stream (§ the two placements of A§9/S§3):
- **Issue-row plane** — `DocMeta.{title,status,priority,assigneeSummary,head}` moved for some
  docs. The TUI re-reads the affected board slice; the row it reads *is* the Loro-truth-derived
  cache (S§3.1), so it already reflects the LWW winner — nothing to compute.
- **Catalog-structure plane** — board *ordering* (`boards[proj]`, e.g. an `IssueMove` reorder,
  which leaves `DocMeta` untouched), project/label config, workflow columns, or the ACL. The
  TUI re-reads that Catalog slice.

**Batching is two-level, each stage grouping at the boundary it uniquely knows:**
- **Daemon (temporal/transactional).** The daemon coalesces changes within a window — a whole
  catalog-first sync-import transaction (A§8), plus a short debounce for rapid local edits —
  into **one** doorbell carrying the unioned dirty set. A single local edit is the degenerate
  case: one doorbell, one doc. This protects the socket and keeps the ring buffer meaningful
  (1000 *batches*, not 1000 individual doc changes). The project keying is **free**: every
  dirty doc's `projectId` is already in hand during the S§3.1 row recompute
  (`get_changed_containers_in`), so partitioning costs the daemon nothing.
- **Client (spatial/visibility).** The TUI intersects `dirty_by_project` with what is on
  screen and re-reads only the visible project's slice; whole off-screen projects are skipped
  with a single map lookup, without parsing their doc lists. **Sync-burst cost is ∝ screen
  size, not workspace size** — the whole point of the catalog-cache design (A§9).

**The feed is pulled, not pushed.** A 300-doc remote import must not stream 300 transition
rows. The doorbell only sets `activity_advanced`; the TUI materializes feed rows lazily via
the existing `Activity { since }` request when the feed view (§5.4) is open — "doorbell rings,
view pulls," consistent all the way through. (A single local edit may carry its one transition
inline for a snappy feed; at scale it is pull.)

**Snapshot model.** Opening an issue fires `IssueView` (`show`), which **lazily loads the
issue doc** daemon-side; body/comments/history live only in the detail view, never the board
model (A§9 lazy body). The board model itself is built from `Board`/`List` — the `DocMeta`
projection — so a 5,000-issue workspace loads from the **one Catalog doc**, not 5,000 issue
docs (A§9 traversal-from-catalog).

### 4.3 Optimistic overlay — correlation-free

The overlay is a **local prediction**, nothing more. An edit keystroke:

1. **Applies an overlay** keyed by `(DocId, field)` → predicted value, and **re-renders
   immediately** — the user sees the change at keystroke latency.
2. **Sends the `Request`** on the command channel.
3. **Clears the overlay on *any* doorbell for that scope** — its own write's echo or a
   concurrent remote edit — by re-reading the authoritative `DocMeta` row. The TUI never
   correlates a doorbell to *its* write; it always yields to the row (which is the LWW winner,
   S§3.1). If the `Request` returns **`Error`**, it rolls the overlay back.

Two properties make this sound, both decided during design review:

- **Validate-then-commit (the write contract).** The daemon fully resolves refs and validates
  a `Request` *before* any Loro commit; on failure it returns `Error` having **touched nothing
  and rung no doorbell**. So `Error` unambiguously means "nothing happened" — rollback is
  race-free. This is clean precisely because there is **no CAS** (S§7.2): the only failures are
  pre-commit (bad ref, unknown project, parse); a well-formed write on a CRDT cannot fail
  *after* commit.
- **Accepted flicker, no op-id.** If a remote doorbell for the same scope lands *before* your
  pending write commits, the overlay clears early (shows the pre-write value), then your write
  lands and re-reads to the merged value — a one-frame flicker that always **converges**. The
  alternative — per-write correlation to clear only on your *own* doorbell — re-adds the op-id
  plumbing the doorbell model exists to delete. We take the rare, convergent flicker;
  same-field concurrent local-pending + remote edit is a millisecond-window event.

The optimism lives in the overlay, the truth lives in the daemon's Loro doc, and the local
IPC hop is fast enough that the overlay is almost always confirmed within a frame — the honest
client-model expression of A§9's "optimistic local ops."

### 4.4 Render loop & coalescing

Event-driven, not a busy loop. The TUI `select!`s over **terminal input** and the **doorbell
stream**, and redraws only when the model or focus changes — idle costs nothing. The render
frame is also the **client coalescing point** (§4.2): doorbells that arrive within a frame are
unioned, so a burst of remote edits triggers **one** set of minimal, visibility-bounded
re-reads and **one** repaint. Rows under an active overlay render with a subtle marker;
`provisional` rows (§3.3) render dimmed; a row whose optimistic edit failed (`Error`) flashes
once as it rolls back.

### 4.5 Daemon lifecycle & presence honesty

A `Subscribe` connection holds `active_conns >= 1` (`node.rs`), so an open TUI **pins the
daemon alive** and idle-shutdown only ever fires in pure-CLI use. **This is intended, not a
leak:** an always-on node is what the P2P design wants more of — it densifies the gossip mesh
and is the on-ramp to the seed role (A§10, "any client node can be promoted to a seed").

The one genuine leak inside that is **false availability** — advertising `● online`
(interactive, reply-ready) while the window is merely parked and you are AFK. So presence is
**three-state**, driven by *input*, not by connection existence:

| State | Meaning | Driven by |
|---|---|---|
| `online` | interactive, reply-ready | TUI/CLI/MCP **input** within the engagement window |
| `away` | node up and syncing, human/agent not engaged | daemon alive, no recent input |
| `offline` | node down | daemon stop / `Bye` / presence lapse |

`PeerState` is binary today (`presence.rs`) and `Payload::Presence` carries only a nick, so
`away` is a **P1 wire change** (a `postcard` bump — all nodes upgrade together, per
HARDENING). It is designed now because `away` is exactly the state HARDENING's **"notify
anyway"** (interrupt tier) is built to punch through: an `away` agent is the canonical target
of an escalated message, so this rung is the P2 receipt/tier model's input, not cosmetics.

## 5. Views

Five views, one modal command palette. Navigation is a stack (`Esc` pops); the board is the
root.

### 5.1 Board — the root view

Columns are `Catalog.workflow` states in order; each column is the rows whose
`Issue.projectId == P` in `boards[P]` order, **deduplicated, belonging-but-unlisted rows
appended, listed-but-not-belonging ignored** (S§5.5 render rule). Done column via the append
rule, wall-clock desc (S§5.7).

```
 ENG · lait                                    [/] filter  [:] cmd  [?] help
 ┌ Backlog ──────┐ ┌ In Progress ──┐ ┌ In Review ────┐ ┌ Done ─────────┐
 │ ENG-142 ·H·   │ │ ENG-140 ·U·▲  │ │ ENG-133 ·M·   │ │ ENG-131       │
 │ parse ticket… │ │ fix login rc… │ │ catalog diff… │ │ seed rooting  │
 │ ○ iss_3f9     │ │ ● you  iss_7a1│ │ ● ab +1 iss_c2│ │ ✓ iss_9e0     │
 │               │ │               │ │               │ │               │
 │ ENG-145 ·L·   │ │ ENG-141 ·H·   │ │               │ │ ENG-128       │
 │ …             │ │ …             │ │               │ │ …             │
 └───────────────┘ └───────────────┘ └───────────────┘ └───────────────┘
  ●=assigned to you  ▲=optimistic  ·U/H/M/L·=priority   3 selected · x
```

- `h`/`l` move focus across columns, `j`/`k` within a column.
- `J`/`K` **reorder** the focused issue within its column — a real board op: `IssueMove`
  with `--before`/`--after` the neighbor, mutating `boards[P]` (the movable list, the native
  win of A§9). Optimistic overlay reorders the row instantly.
- `H`/`L` **move status**: `IssueEdit --status` to the prev/next workflow column.
- Quick actions on the focused issue: `a` assign, `l` label, `p` priority, `m` move project,
  `s` set status (picker), `Enter` open detail, `x` toggle multi-select (then the same keys
  act on the selection — one `Request` per issue).

### 5.2 List

A flat, dense, filterable table (the `ls` view). Same rows, no columns; sortable by
priority/updated/created. Good for triage and for `--mine`. Shares all quick-action keys
with the board.

### 5.3 Issue detail

Lazy-loaded via `IssueView`. Title, `description` (rendered `LoroText`), metadata
(project/status/priority/assignees/labels), comments, and a collapsible **activity feed**
(the derived history, A§5).

```
 ENG-140  fix login race condition                      iss_7a1  ·In Progress·
 ─────────────────────────────────────────────────────────────────────────────
 Priority Urgent     Assignees ● you, ab      Labels  bug, auth
 Project  ENG                                 Created ab · 2026-07-08

 ## Description
 The token refresh and the initial auth race when… (LoroText, co-editable)

 ## Comments (2)
 ab · 09:14   repro is flaky, ~1 in 5 cold starts
 you · 09:31  looks like the refresh lock isn't held across the await

 ## Activity
 09:31 you   status  In Review → In Progress   ⚠ concurrent with ab's edit
 09:14 ab    comment added
 ─────────────────────────────────────────────────────────────────────────────
 [e]dit title  [d]escription  [C]omment  [a]ssign  [l]abel  [t]imeline  [Esc]
```

- `e` edit title (single-line register, LWW — S§5.1), `d` edit description (opens a
  multi-line editor; on P0 a full-buffer replace, since the client holds no `LoroText`
  cursor — the daemon applies it as a text update).
- `C` comment, `t` expand the full time-travel timeline (A§5), `y` yank the ref.

### 5.4 Activity (workspace feed)

Every transition across the workspace, newest first, ordered by `seq`/wall-clock (advisory,
S§2), **never by Lamport** (that would be unreadable — S§2's two-orderings rule). This is
where LWW collision notes (A§9) surface as `⚠` lines. **The feed is pulled, not pushed
(§4.2):** the doorbell stream only sets `activity_advanced` ("there are new rows"); when this
view is open it materializes rows lazily via `Activity { since }`, so a 300-doc remote import
never floods the stream with 300 transition frames — it rings once and the view pulls what it
can show.

### 5.5 Command palette

`:` (or `Ctrl-K`) opens a fuzzy palette over **commands + issues + projects**: type
`assign ENG-140 @me`, or jump to an issue by title/ref. Every CLI verb is reachable here, so
the palette is the CLI grammar (§2, §3) with fuzzy completion — one grammar, two entry
points (tenet 4).

### 5.6 Pickers & disambiguation

Assign/label/project/status open a fuzzy picker over the relevant registry. A ref that
resolves to **many** candidates (§3.2) opens a disambiguation picker rather than erroring.

## 6. Keymap

Vim-familiar motion, Linear-familiar actions. Global keys work in every view; view-specific
keys layer on top.

| Scope | Key | Action |
|---|---|---|
| Global | `?` | help overlay |
| Global | `:` / `Ctrl-K` | command palette (§5.5) |
| Global | `/` | filter / search |
| Global | `q` / `Esc` | pop view / quit at root |
| Global | `r` | force snapshot reload (self-heal) |
| Global | `1`/`2`/`3` | board / list / activity view |
| Motion | `j`/`k` `h`/`l` | move focus (down/up, col left/right) |
| Motion | `g`/`G` | top / bottom |
| Board | `J`/`K` | reorder issue within column (`IssueMove`) |
| Board | `H`/`L` | move issue to prev/next status |
| Issue op | `c` | create issue (quick-create modal) |
| Issue op | `Enter` | open detail |
| Issue op | `a`·`l`·`p`·`m`·`s` | assign · label · priority · move · status |
| Issue op | `x` | toggle multi-select |
| Detail | `e`·`d`·`C`·`t`·`y` | edit title · description · comment · timeline · yank ref |

**Quick-create (`c`)** is a single modal: title line, then optional `-p`/`-a`/`-P`/`-l`
inline tokens parsed with the same grammar as `new` (§2). One `Enter` = one `IssueNew` = one
issue = one activity row.

## 7. Conflict & limitation surfacing

The UI must make the CRDT's honest limitations legible rather than hiding them:

- **LWW collisions** on `status`/`priority`/`title` (A§9, S§5.1) never block. The losing
  write lands, and a **non-blocking `⚠` activity note** appears in the feed and on the
  detail view's activity section ("status In Review → In Progress, concurrent with ab").
  The board just shows the merged value.
- **No CAS (S§7.2)** — the TUI offers no "close only if open" affordance; an action always
  applies and merges. If the world moved under you, the doorbell stream repaints the new truth.
- **Convergent flicker (§4.3)** — a remote edit racing your pending optimistic write on the
  same field can flicker the value for one frame before converging. Accepted: it always
  settles, and avoiding it would re-add per-write correlation the doorbell model deletes.
- **Provisional / self-healing rows (§3.3)** render dimmed with a marker; no error, no
  spinner-forever — they fill in when the issue doc arrives.
- **`KEY-n` disambiguation (S§5.4)** surfaces as the suffix (`ENG-142b`) beside the
  canonical `iss_` handle plus an activity note, never as a silent renumber.
- **Attribution is advisory (A§ non-goal 6).** Authorship (`createdBy`, comment authors) is
  shown as data, not a verified badge; the UI does not imply cryptographic provenance.

## 8. Forward hooks (P1+) — slotted onto the P0 grammar

The P0 surface was designed so later phases **add panels and columns, never reshape the
grammar** — and that held: P1 (live sync/presence) and P3 (membership) landed without
touching the issue grammar. Where each attaches:

- **P1 — live sync & presence.** A status bar gains a **sync indicator** (peers online,
  catalog-head freshness, "syncing N docs") fed by the existing presence/gossip events
  (A§8); `who`/`invite`/`connect` become the TUI's peers panel.
  No new issue grammar — sync is ambient.
- **P1/P2 — receipts & tiers ([`HARDENING.md`]).** `send`/`ack`/`receipts`/`focus` and the
  tier ladder (`ambient…interrupt`) attach to the **activity/notification** surface, not the
  issue model: `watch --min-tier/--on-interrupt` is the CLI teeth; the TUI shows receipt
  badges (`✓delivered ✓seen ✓acked`) and honors `mute_below`. Designed there, slotted here.
- **P3 — membership UI (landed).** A **members view** over `Catalog.acl` (S§6): roles,
  add/remove, key rotation, driven by `MemberAdd/Remove`, `KeyRotate` (S§7). The ACL is
  the only signed structure, so this view is the only one showing verified identity.
  Join-request approval rides on the same op-graph: `members
  requests` lists announced joiners (authenticated key + an *unverified* nick claim) and
  `members approve <prefix|key> [--as <name>]` signs the `AddMember` op — resolving
  **key-first**, never by the self-asserted nick (an unauthenticated name must not select
  who is sealed the workspace key). By **default** this manual step is collapsed: an
  `invite` ticket carries a signed, single-use **pass** (S§6.1) and the joiner is
  auto-admitted on `join` — the admin node still signs the same `AddMember` op and seals
  key-side, so E2EE is unchanged; the pass only removes the keystroke. `invite
  --require-approval` mints a pass-less ticket for the human-in-the-loop flow above;
  `--reusable`/`--ttl-hours` tune a pass. Friendly names are **local aliases** (petnames): a
  key is the identity, and `<userref>` (§3.1) resolves an alias/prefix against your own alias
  store — never a wire nick.
- **P4 — MCP parity & polish.** The MCP tool set (A§12) is generated from / checked against
  the **same `Response` DTOs** the CLI `--json` emits (S§7.3), so agent and human surfaces
  never drift. TUI polish (themes, resize, wide-table horizontal scroll) is P4.

## 9. Decisions — settled (mirror of A§14 / S§10)

- **§4 TUI substrate — ratatui** (default, agreed) vs any other Rust TUI lib. Settled.
- **§4.1 live channel — streaming `Subscribe`** (default) vs re-polling `Wait`. Originally
  settled as "both supported" (Subscribe for the TUI, Wait for scripting/`watch`); **revised**
  once the doorbell grew a presence plane — `watch` now rides `Subscribe` and `Wait` is
  deleted. One wake path, one rebaseline story (`Reset`/`epoch`).
- **§4.2 event shape — batched, project-keyed doorbells** (agreed) vs value-carrying deltas.
  Settled: doorbells carry a dirty-set, never state; the client re-reads.
- **§4.3 reconciliation — correlation-free, accept the flicker** (agreed) vs op-id-correlated
  overlays that clear only on their own write. Settled in favor of no correlation.
- **§4.1 cursor — ephemeral `seq` + `Reset`-doorbell rebaseline** (agreed) vs a durable `seq`
  persisted across daemon restarts. Settled: `seq` is per-session; `Reset` handles every gap.
- **§4.5 presence — three-state (`online`/`away`/`offline`), input-driven** (agreed). The
  `away` rung is a P1 `postcard` wire bump (all nodes upgrade together) and the P2 tier input.
- **§4.2 daemon debounce window** — the coalescing window length for rapid local edits
  (impl detail, a few ms) — deferred to build.
- **P1 feed flood** — whether a large remote import coarsens the pulled feed (§5.4) or lists
  every transition — deferred; the doorbell already prevents the *stream* flood, so this is a
  feed-rendering choice only.
- **§5.3 description editing — full-buffer replace at P0** (default; client holds no
  `LoroText` cursor) vs an in-TUI collaborative-cursor editor (later; needs the client to
  hold a live `LoroText` view, which reintroduces a client-side replica — deferred with the
  in-process question).
- **§5.5 palette key — `:` and `Ctrl-K`** (default, both bound) — trivially flippable.
- **CLI verb layout — flat verbs act on issues, plural nouns manage registries** (default,
  agreed) so `label <ref> +bug` (issue op) and `labels new` (registry) never collide.

## 10. Decision log

- **All three surfaces are Layer-B clients of the one daemon** — the TUI is a **client over
  IPC**, not an embedded node. "Optimistic render" is client-side echo over a local hop
  (§4.3), so there is one Loro owner, one contract to stabilize, and the TUI inherits the
  refactor-freedom Layer B buys the CLI and MCP (§1). Rejected: a TUI that holds its own Loro
  replica (a second source of truth, the exact hazard S§3 removes).
- **One ref grammar, resolved daemon-side** — `Ref`/`UserRef` mean the same thing in the
  CLI, palette, and MCP; ambiguity (short prefix / colliding `KEY-n`) is a first-class
  outcome with a candidate list, not a crash (§3). Canonical handle is always the short
  `DocId`; `KEY-n` is a friendly alias (S§5.4).
- **`Subscribe` is the one live Layer-B verb** — the single streaming wake path for the TUI
  *and* CLI `watch`; the `Wait` long-poll it superseded is deleted (§4.1). The rest of the TUI
  is built from `Board`/`List`/`IssueView` + the doorbell stream over the S§7 surface — no new
  domain schema.
- **The event stream is doorbells, not deltas** — a frame rings "these scopes are dirty," the
  client re-reads the Loro-truth-derived projection (S§3.1); it never carries state. This is
  what dissolves reconciliation: no op-id, no embedded payload, no partial patch. The LWW
  winner is adopted for free because `DocMeta` *is* the winner (§4.2–§4.3). Rejected: fat
  events carrying the resulting row + a client op-id — the schema already materializes the row,
  so both were reinventing S§3.1.
- **Reconciliation is correlation-free and validate-then-commit** — the overlay is a local
  prediction cleared by *any* doorbell for its scope; `Error` guarantees nothing committed
  (no CAS, S§7.2), making rollback race-free; a rare remote-vs-pending flicker is accepted
  because it converges (§4.3).
- **Doorbells are batched two-level and project-keyed for free** — the daemon coalesces by
  sync-transaction + debounce into one set-valued frame (protecting the socket and the 1000-
  entry ring, which now holds *batches*); the client filters by visibility, so sync-burst cost
  is ∝ screen, not workspace. Project keying falls out of the S§3.1 `DocMeta` recompute at no
  cost (§4.2). The feed is pulled via `Activity{since}`, never streamed row-by-row (§5.4).
- **The cursor is ephemeral; `Reset` unifies every gap** — `seq` is per-daemon-session (S§2
  reworded), so first-connect, reconnect, restart, and ring-overrun all collapse to one "snapshot
  + rebaseline" path signalled by a `Reset` doorbell + a per-boot epoch nonce (§4.1). `seq`
  never needs persisting. This also fixes a pre-existing `watch` deafness across
  the routine idle-shutdown (the old `Wait` poll loop held a stale cursor with no epoch to
  void it).
- **A `Subscribe`-pinned daemon is a feature; the only leak is false availability** — an open
  TUI keeps the node alive, densifying the mesh toward the seed role (A§10). Honesty is restored
  by input-driven three-state presence (`online`/`away`/`offline`), and `away` is precisely
  HARDENING's "notify anyway" target (§4.5).
- **Board reorder is a real `IssueMove`, board status-move is `IssueEdit`** — the movable
  list `boards[P]` is the ordering truth (A§9, S§5.5); the TUI mutates it directly, and
  `Issue.projectId` remains the single membership source (S§5.5). No rank field on issues.
- **The UI surfaces CRDT honesty** — LWW collisions, no-CAS, provisional rows, advisory
  attribution are shown, not hidden (§7), matching the accepted limitations in A§3/S§3.
- **Verbs are drawn at commit boundaries** — one command = one `Request` = one Loro commit =
  one activity row (§1 tenet 2, S§7.1), which is what keeps the free derived history (A§5)
  readable.
- **P0-complete, forward-slotted** — sync/presence (P1), receipts/tiers (P2, HARDENING),
  members (P3), MCP-parity (P4) attach as panels to a grammar fixed now (§8), matching the
  no-wire-rework discipline of A§10/A§13.

**Companion sources:** [`ARCHITECTURE.md`](./ARCHITECTURE.md) (A§) ·
[`SCHEMA.md`](./SCHEMA.md) (S§) · [`HARDENING.md`](./HARDENING.md) (receipts/tiers) ·
[`GUIDED-JOIN.md`](./GUIDED-JOIN.md) (onboarding) · [ratatui](https://ratatui.rs) ·
`src/control.rs`, `src/cli.rs`, `src/tui/`.
