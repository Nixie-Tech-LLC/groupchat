import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Group, Panel, Separator, useDefaultLayout, usePanelRef } from "react-resizable-panels";
import {
  Inbox as InboxIcon,
  LayoutGrid,
  List,
  ListFilter,
  PanelLeft,
  Plus,
  Search,
} from "lucide-react";

import { ConfirmRequired, LaitError, rpc, spaces as fetchSpaces } from "./api";
import { useDoorbell } from "./doorbell";
import { coalesce } from "./core/coalesce";
import { groupRows, loadDisplay, saveDisplay, type DisplayState } from "./core/display";
import {
  contribute,
  registry,
  type AppApi,
  type Ctx,
  type IssueField,
  type View,
} from "./core/registry";
import { useKeys } from "./core/useKeys";
import { neighbourState, workTarget } from "./core/workflow";
import { Activity } from "./ui/Activity";
import { Board } from "./ui/Board";
import { BulkBar } from "./ui/BulkBar";
import { DisplayOptions } from "./ui/DisplayOptions";
import { FilterBar } from "./ui/FilterBar";
import { Inbox } from "./ui/Inbox";
import { Members } from "./ui/Members";
import { IssueDetail } from "./ui/IssueDetail";
import { IssueList } from "./ui/IssueList";
import { RolesDialog, WorkflowDialog } from "./ui/Governance";
import { NewIssue } from "./ui/NewIssue";
import { NewProject } from "./ui/NewProject";
import { Palette } from "./ui/Palette";
import { Shortcuts } from "./ui/Shortcuts";
import { catalogColor } from "./ui/colors";
import * as ask from "./ui/dialogs";
import { DialogHost } from "./ui/dialogs";
import { Combobox } from "./ui/Picker";
import { IconButton, TooltipProvider } from "./ui/primitives";
import { Sidebar } from "./ui/Sidebar";
import {
  applyFilter,
  EMPTY_FILTER,
  isActive,
  needsServer,
  type FilterState,
} from "./core/filter";
import { applyOverlay, Overlay, PREDICTION_TTL_MS, type Field } from "./core/overlay";
import {
  isReadOnly,
  type BoardPos,
  type BoardView,
  type LabelDto,
  type MemberDto,
  type ProjectDto,
  type Row,
  type SpaceRow,
  type WorkflowState,
} from "./types";
import "./commands";

type Modal = "palette" | "shortcuts" | "workflow" | "roles" | null;

/**
 * The shell.
 *
 * It owns state and supplies an [`AppApi`]; it does not own keys. Every gesture —
 * a shortcut, a palette entry, a button — resolves to a command id and runs it, so
 * a behaviour is defined once and is overridable in one place. Buttons call
 * `registry.get(id)?.run(ctx)` rather than a local handler, which is what stops
 * "click" and "keypress" from drifting apart.
 */
export function App() {
  const [spaces, setSpaces] = useState<SpaceRow[]>([]);
  const [current, setCurrent] = useState<string | null>(null);
  const [board, setBoard] = useState<BoardView | null>(null);
  const [selection, setSelection] = useState<string | null>(null);
  const [modal, setModal] = useState<Modal>(null);
  const [error, setError] = useState<string | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  const [detail, setDetail] = useState(true);
  const [view, setView] = useState<View>("list");
  const [unread, setUnread] = useState(0);
  /** The composer, and the column it was opened from (null = closed). */
  const [composing, setComposing] = useState<{ status?: string } | null>(null);
  const [composingProject, setComposingProject] = useState(false);
  const [filter, setFilter] = useState<FilterState>(EMPTY_FILTER);
  const [filterOpen, setFilterOpen] = useState(false);
  const [focusToken, setFocusToken] = useState(0);
  /** Group / order / show-deleted. Loaded once; every change is persisted. */
  const [display, setDisplay] = useState<DisplayState>(loadDisplay);
  const [displayOpen, setDisplayOpen] = useState(false);
  /** Bulk-selection checks, by canonical ref. Distinct from `selection`: the
   *  focus is one row, the checks are a set, and `x` is the bridge. */
  const [checked, setChecked] = useState<ReadonlySet<string>>(new Set());
  const [labels, setLabels] = useState<LabelDto[]>([]);
  const [members, setMembers] = useState<MemberDto[]>([]);
  const [projects, setProjects] = useState<ProjectDto[]>([]);
  /** Which project's board is on screen. `null` = let the daemon's chain pick
   *  (branch key → `project.default` → the only project), same as a bare `lait board`. */
  const [project, setProject] = useState<string | null>(null);
  /** The picker a keybinding has asked for. Also an overlay: it owns the keymap. */
  const [field, setField] = useState<IssueField | null>(null);
  /** Doc-ids the daemon says qualify. `null` = the daemon wasn't asked, which is
   *  not the same as "nothing qualifies" — see core/filter.ts. */
  const [allowed, setAllowed] = useState<ReadonlySet<string> | null>(null);
  /** Tombstoned rows, fetched only while the display option shows them.
   *  Deleting an issue REMOVES it from `boards[P]` (the board genuinely does
   *  not know it), so the trash comes from `list all:true`, not the board. */
  const [deletedRows, setDeletedRows] = useState<Row[]>([]);
  /** Local predictions. A ref, not state: the doorbell handler mutates it and we
   *  re-render explicitly — putting it in state would make every `set` a new Map
   *  and every render a new overlay. */
  const overlay = useRef(new Overlay());
  const [predicted, setPredicted] = useState(0);
  /** Monotonic load token — see `loadBoard`. A generalisation of an `alive` flag:
   *  it also orders two loads of the *same* space, which `alive` cannot. Bumped
   *  when a load is *requested*, not when it starts: once requests coalesce, the
   *  request is the thing that supersedes, and a run that starts later already
   *  carries the newer args. */
  const boardSeq = useRef(0);
  /** Last doorbell epoch seen per space — the daemon-boot nonce (UI.md §4.1). */
  const epochs = useRef(new Map<string, number>());
  // Bumped on every doorbell for this space: the detail pane re-reads off it.
  const [revision, setRevision] = useState(0);
  const sidebar = usePanelRef();

  const space = spaces.find((s) => s.id === current) ?? null;
  const readOnly = space ? isReadOnly(space) : false;

  // Overlay first, then filter: a predicted title should be findable by the text
  // you just typed into it, and a predicted status should filter as its new one.
  // `predicted` is the re-render trigger — the overlay itself is a mutable ref.
  const { shown, optimistic } = useMemo(() => {
    if (!board) return { shown: null, optimistic: new Set<string>() as ReadonlySet<string> };
    const o = applyOverlay(board, overlay.current);
    return { shown: applyFilter(o.board, filter, allowed), optimistic: o.optimistic };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [board, filter, allowed, predicted]);

  /** The list's arrangement (the board renders columns straight off `shown`). */
  const groups = useMemo(() => (shown ? groupRows(shown, display) : []), [shown, display]);

  // Motion follows what is *visible*, in the order it is visible: on the list,
  // j/k walks the display *groups*; on the board — which always lays out by
  // status regardless of the grouping option — it walks the columns. The trash
  // rows join the motion exactly when the display option shows them — a row you
  // can see but not land on is a trap.
  const rows: Row[] = useMemo(() => {
    const live =
      view === "board" && shown
        ? shown.columns.flatMap((c) => c.rows.filter((r) => !r.tombstone))
        : groups.flatMap((g) => g.rows.filter((r) => !r.tombstone));
    return display.deleted ? [...live, ...deletedRows] : live;
  }, [view, shown, groups, display.deleted, deletedRows]);

  // Persisted like the sidebar width: an arrangement chosen once should hold.
  useEffect(() => {
    saveDisplay(display);
  }, [display]);

  const loadSpacesRaw = useCallback(async () => {
    try {
      const { spaces } = await fetchSpaces();
      setSpaces(spaces);
      setError(null);
      setCurrent((cur) => {
        if (cur) return cur;
        // Attaching an agent brings that agent *online*, so auto-select only our
        // own single unambiguous space — never an agent.
        const mine = spaces.filter((s) => !isReadOnly(s));
        return mine.length === 1 && mine[0] ? mine[0].id : null;
      });
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  /**
   * Load the board, and keep trying.
   *
   * A failed load must not be terminal. The daemon this space talks to can
   * restart under us — someone runs `lait shutdown`, an update swaps the binary,
   * two processes race to respawn it — and the failure lasts milliseconds. But
   * nothing would re-trigger the load: doorbells arrive through the very
   * attachment that just broke, so a transient error froze the view and left a
   * stale banner over it until the user thought to press `r`. The error was
   * honest; its permanence was the bug.
   *
   * Backs off rather than hammering, and gives up after a few tries so a genuinely
   * dead space says so instead of spinning forever.
   */
  const loadBoardRaw = useCallback(async (id: string | null, proj: string | null): Promise<void> => {
    // Only the newest load may commit. Two doorbells in quick succession issue
    // two loads, and the one that resolves *last* is not the one issued last —
    // so an older board could silently overwrite a newer one, and nothing would
    // correct it until the next ring. The retry below makes it worse by holding a
    // load open for ~2.8s, long enough to land on a space you have since left and
    // paint its error over the one you are looking at.
    const seq = boardSeq.current;
    const stale = () => seq !== boardSeq.current;

    if (!id) return setBoard(null);
    for (let attempt = 0; ; attempt++) {
      try {
        const r = await rpc(id, { cmd: "board", project: proj });
        if (stale()) return;
        setBoard(r.kind === "board" ? r : null);
        setError(null);
        return;
      } catch (e) {
        if (stale()) return;
        if (attempt < 3) {
          await new Promise((r) => window.setTimeout(r, 400 * 2 ** attempt));
          if (stale()) return;
          continue;
        }
        setBoard(null);
        setError(e instanceof Error ? e.message : String(e));
        return;
      }
    }
  }, []);

  /**
   * The two re-reads a doorbell fans out to, coalesced.
   *
   * A ring is per commit, not per user action, so a sync burst asked for the same
   * board ten times in a couple hundred milliseconds. The `seq` guard kept the
   * *answers* honest, but the questions were all still asked. See `core/coalesce.ts`
   * for why this is one-in-flight-plus-one-trailing rather than a plain throttle —
   * the trailing run is what makes the read postdate the news that provoked it.
   *
   * Bumping `boardSeq` here rather than inside the run is what lets a queued request
   * cut a doomed one short: a load for the space you just left sees `stale()` at its
   * next check and returns without painting, and a retry chain mid-backoff stops
   * waiting out its ~2.8s once there is a newer question to answer.
   */
  const loadBoard = useMemo(() => {
    const run = coalesce(loadBoardRaw);
    return (id: string | null, proj: string | null): Promise<void> => {
      boardSeq.current++;
      return run(id, proj);
    };
  }, [loadBoardRaw]);
  const loadSpaces = useMemo(() => coalesce(loadSpacesRaw), [loadSpacesRaw]);

  // The project is read through a ref by the doorbell handler and the sweep, which
  // must not re-subscribe every time it changes.
  const projectRef = useRef(project);
  projectRef.current = project;

  useEffect(() => {
    void loadSpaces();
  }, [loadSpaces]);
  useEffect(() => {
    void loadBoard(current, project);
  }, [current, project, loadBoard]);

  // A project selected in one space does not exist in the next one.
  useEffect(() => {
    setProject(null);
  }, [current]);

  /**
   * Name a project once we know there is a choice.
   *
   * `Request::Board { project: null }` asks the daemon to resolve one, and its
   * chain is a **CLI** chain: the git branch's key → `project.default` → the only
   * project → a teaching error. A browser tab has no cwd and no branch, so on a
   * space with more than one project that chain reaches the error every time — and
   * the client sent `null` unconditionally, which is why a second project made the
   * board render "more than one project (ACME, DSN) — pass -p <KEY>" instead of
   * issues. The switcher in the header is only half the fix; this is the other.
   *
   * Left `null` for a single-project space on purpose: the chain resolves it fine,
   * and `project.default` keeps working for the one case a browser can honour it.
   * (Reading `project.default` outright is not possible from here — `config` is a
   * `Special` CLI handler, not a `Request`, so no HTTP endpoint reaches it.)
   */
  useEffect(() => {
    if (project !== null || projects.length < 2) return;
    setProject(projects[0]!.key);
  }, [projects, project]);

  /**
   * The three registries every picker reads from — the daemon's, never ours.
   *
   * Fetched together because they share a lifetime (this space, this revision) and
   * a failure mode: none of them is worth an error banner. A picker with fewer
   * options is a smaller menu; a red bar across the board is a broken app. The
   * board is the thing whose failure is worth shouting about, and it already does.
   *
   * Same race as `loadBoard`: switch space mid-flight and the old space's members
   * would land in the new space's assignee picker — hence `alive`.
   */
  useEffect(() => {
    if (!current) {
      setLabels([]);
      setMembers([]);
      setProjects([]);
      return;
    }
    let alive = true;
    void (async () => {
      const [l, m, p, s] = await Promise.all([
        rpc(current, { cmd: "label_list" }).catch(() => null),
        rpc(current, { cmd: "members" }).catch(() => null),
        rpc(current, { cmd: "project_list" }).catch(() => null),
        rpc(current, { cmd: "status" }).catch(() => null),
      ]);
      if (!alive) return;
      if (l?.kind === "labels") setLabels(l.labels);
      if (p?.kind === "projects") setProjects(p.projects);
      if (m?.kind === "members") {
        // `members` carries no alias for **you**: a petname is something you assign
        // to other people, so `replica.rs::members` reports `alias: ""` for `me`.
        // Your own name lives in `user.nick`, which only `status` reports — and
        // without it yours is the one avatar in the space with no letter on it,
        // which is a strange way to meet yourself. Patched here rather than in the
        // avatar, so every surface agrees on what you are called.
        const nick = s?.kind === "status" ? s.nick.trim() : "";
        setMembers(
          nick ? m.members.map((x) => (x.me && !x.alias ? { ...x, alias: nick } : x)) : m.members,
        );
      }
    })();
    return () => {
      alive = false;
    };
  }, [current, revision]);

  // The trash. Scoped to the board's project so the group matches the view,
  // re-read on every doorbell (a remote delete is exactly the news it carries).
  useEffect(() => {
    if (!current || !display.deleted) return setDeletedRows([]);
    let alive = true;
    void (async () => {
      try {
        const r = await rpc(current, {
          cmd: "list",
          project: board?.project.key ?? null,
          filter: { all: true },
        });
        if (alive && r.kind === "list") setDeletedRows(r.rows.filter((x) => x.tombstone));
      } catch {
        if (alive) setDeletedRows([]);
      }
    })();
    return () => {
      alive = false;
    };
  }, [current, display.deleted, board?.project.key, revision]);

  // `mine`/`label` are server truth: ask `list`, keep the doc-ids, intersect.
  useEffect(() => {
    if (!current || !needsServer(filter)) return setAllowed(null);
    let alive = true;
    void (async () => {
      try {
        const r = await rpc(current, {
          cmd: "list",
          project: null,
          filter: { mine: filter.mine, label: filter.label, all: true },
        });
        if (alive && r.kind === "list") setAllowed(new Set(r.rows.map((x) => x.doc_id)));
      } catch (e) {
        if (alive) setError(e instanceof Error ? e.message : String(e));
      }
    })();
    return () => {
      alive = false;
    };
  }, [current, filter, revision]);

  // A selection that no longer exists (deleted, filtered away) must not linger.
  useEffect(() => {
    setSelection((s) => (s && rows.some((r) => r.reff === s) ? s : (rows[0]?.reff ?? null)));
  }, [rows]);

  // Checks on rows that left the view are stale writes waiting to happen: a
  // bulk action must only ever touch what the user can currently see checked.
  useEffect(() => {
    setChecked((c) => {
      const live = new Set([...c].filter((reff) => rows.some((r) => r.reff === reff)));
      return live.size === c.size ? c : live;
    });
  }, [rows]);

  useEffect(() => {
    if (!toast) return;
    const t = window.setTimeout(() => setToast(null), 2400);
    return () => window.clearTimeout(t);
  }, [toast]);

  const liveness = useDoorbell(
    useCallback(
      (d) => {
        if (!d) {
          // We can't say which docs moved, so no prediction can be trusted.
          overlay.current.clear();
          setPredicted((n) => n + 1);
          void loadSpaces();
          void loadBoard(current, projectRef.current);
          setRevision((r) => r + 1);
          return;
        }
        // `epoch` is a per-daemon-boot nonce: a change means that daemon
        // restarted, so our position in its stream is meaningless and nothing we
        // hold about the space is trustworthy — which is exactly what `reset`
        // says (UI.md §4.1). The server sends `reset` on the death it can see;
        // the epoch catches a restart it can't, where a daemon dies and returns
        // between two frames. Recorded for every space, not just the selected
        // one, so switching to a space doesn't compare against a stale nonce.
        const prev = epochs.current.get(d.space);
        epochs.current.set(d.space, d.epoch);
        const rebaseline = d.reset || (prev !== undefined && prev !== d.epoch);

        if (d.space !== current) return;
        // The doorbell is the spine of the optimism: it names the docs that
        // moved, and the arrival of truth about a doc is what kills every guess
        // about it — no ids to match, nothing to reconcile. Re-read FIRST, then
        // drop the predictions: clearing before the fresh rows land would flash
        // the old server value for a frame, which is the one thing the optimism
        // exists to prevent.
        void loadBoard(current, projectRef.current).then(() => {
          const docs = Object.values(d.dirty_by_project).flat();
          let cleared = false;
          for (const doc of docs) cleared = overlay.current.clearDoc(doc) || cleared;
          if (rebaseline) {
            overlay.current.clear();
            cleared = true;
          }
          if (cleared) setPredicted((n) => n + 1);
        });
        setRevision((r) => r + 1);
        // On a rebaseline the space list is exactly as suspect as the board: a
        // daemon that restarted may have changed its own name, projects, or
        // whether it is up at all.
        if (rebaseline || d.dirty_catalog.length) void loadSpaces();
      },
      [current, loadBoard, loadSpaces],
    ),
  );

  /** The workflow, in board order — which is the order the work verbs resolve by. */
  const states: WorkflowState[] = useMemo(
    () => board?.columns.map((c) => c.state) ?? [],
    [board],
  );

  const currentRef = useRef(current);
  currentRef.current = current;
  const rowsRef = useRef(rows);
  rowsRef.current = rows;
  const selRef = useRef(selection);
  selRef.current = selection;
  const statesRef = useRef(states);
  statesRef.current = states;
  const checkedRef = useRef(checked);
  checkedRef.current = checked;
  const membersRef = useRef(members);
  membersRef.current = members;
  // The *filtered* board: reordering has to land relative to a neighbour you can
  // actually see, or `J` jumps the card past rows a filter is hiding.
  const shownRef = useRef(shown);
  shownRef.current = shown;

  /** The selected row, or null. Read through refs so commands stay stable. */
  const selectedRow = useCallback(
    (): Row | null => rowsRef.current.find((r) => r.reff === selRef.current) ?? null,
    [],
  );

  /**
   * Predict, then send.
   *
   * The order is the point: the value is on screen before the request leaves, and
   * the doorbell — not a response — is what retires the guess. If the request is
   * refused we roll back immediately rather than wait for a doorbell that will
   * never come, because a refusal *is* the news.
   */
  const predict = useCallback(
    async (doc: string, field: Field, value: string, send: () => Promise<unknown>) => {
      overlay.current.set(doc, field, value);
      setPredicted((n) => n + 1);
      try {
        await send();
      } catch (e) {
        overlay.current.clearDoc(doc);
        setPredicted((n) => n + 1);
        if (!(e instanceof ConfirmRequired)) {
          setError(e instanceof LaitError ? e.message : String(e));
        }
      }
    },
    [],
  );

  /** Writes never refetch — the daemon rings and the doorbell reloads. */
  const guard = useCallback(async (fn: () => Promise<unknown>) => {
    try {
      await fn();
    } catch (e) {
      if (e instanceof ConfirmRequired) return;
      setError(e instanceof LaitError ? e.message : String(e));
    }
  }, []);

  /**
   * One request per checked issue, in check order, sequentially.
   *
   * Sequential on purpose: each write is its own commit and its own doorbell,
   * and a parallel burst of N mutations against one daemon buys nothing but
   * interleaved activity rows. The first refusal stops the run and shows why —
   * continuing past an error would leave "which of my 12 landed?" unanswerable.
   */
  const bulk = useCallback(async (fn: (reff: string) => Promise<unknown>) => {
    const targets = rowsRef.current.filter((r) => checkedRef.current.has(r.reff));
    for (const row of targets) {
      try {
        await fn(row.reff);
      } catch (e) {
        setError(e instanceof LaitError ? e.message : String(e));
        return;
      }
    }
  }, []);

  const api: AppApi = useMemo(
    () => ({
      openPalette: () => setModal("palette"),
      closePalette: () => setModal(null),
      toggleShortcuts: () => setModal((m) => (m === "shortcuts" ? null : "shortcuts")),
      toggleDetail: () => setDetail((d) => !d),
      goto: (v) => setView(v),
      openFilter: () => {
        setFilterOpen(true);
        setFocusToken((t) => t + 1);
      },
      toggleSidebar: () => {
        const p = sidebar.current;
        if (!p) return;
        if (p.isCollapsed()) p.expand();
        else p.collapse();
      },
      toast: (m) => setToast(m),
      refresh: () => {
        void loadSpaces();
        void loadBoard(current, projectRef.current);
        setToast("Refreshed");
      },
      select: (reff) => setSelection(reff),
      predict: (doc, field, value, send) => void predict(doc, field, value, send),
      pickSpace: (id) => setCurrent(id),
      pickProject: (key) => setProject(key),

      // A picker needs its subject visible: opening the assignee menu over a pane
      // you closed is a menu with no context.
      openField: (f) => {
        setDetail(true);
        setField(f);
      },

      /**
       * A work verb: one `Request`, one commit.
       *
       * Only `status` is predicted. The verbs also bundle assignment in the same
       * commit (`start` takes the issue, `stop` puts it down), but `Row` carries
       * `assignee_summary` — a string the *daemon* derives ("you", "alice +1") —
       * and re-deriving it here to predict it would be a second implementation of a
       * server rule for the sake of one frame. The doorbell brings the real one.
       */
      work: (action) => {
        const row = selectedRow();
        if (!row || !current) return;
        const cmd = `issue_${action}` as const;
        const target = workTarget(statesRef.current, action);
        if (!target) {
          // No state in that category — the daemon refuses with a better sentence
          // than we could write. Send it and show its words.
          void guard(() => rpc(current, { cmd, reff: row.reff }));
          return;
        }
        void predict(row.doc_id, "status", target.id, () =>
          rpc(current, { cmd, reff: row.reff }),
        );
      },

      /** `H`/`L` — the neighbouring workflow column. Clamps at both ends. */
      shiftStatus: (delta) => {
        const row = selectedRow();
        if (!row || !current) return;
        const next = neighbourState(statesRef.current, row.status, delta);
        if (!next) return;
        void predict(row.doc_id, "status", next.id, () =>
          rpc(current, { cmd: "issue_edit", reff: row.reff, status: next.id }),
        );
      },

      /**
       * `J`/`K` — reorder within the column.
       *
       * Position is `Catalog.boards[P]`'s to decide (A§9) and is not a field `Row`
       * carries, so there is nothing to predict: the doorbell repaints. Against a
       * daemon on a Unix socket that is a few milliseconds.
       *
       * Refused in a Done column, and that is not a nicety. Entering a done-category
       * status **removes the doc from `boards[P]`** (`replica.rs:858-869`); done
       * columns are rendered by the append rule instead, sorted `created_at desc`.
       * So a reorder there mutates a list the column isn't drawn from — the request
       * succeeds, the daemon rings, and the card lands exactly where it was. Doing
       * nothing is the honest outcome.
       */
      reorder: (delta) => {
        const row = selectedRow();
        const shownBoard = shownRef.current;
        if (!row || !current || !shownBoard) return;
        const col = shownBoard.columns.find((c) => c.state.id === row.status);
        if (!col || col.state.category === "done") return;

        const visible = col.rows.filter((r) => !r.tombstone);
        const i = visible.findIndex((r) => r.reff === row.reff);
        const target = visible[i + delta];
        if (i < 0 || !target) return;

        void guard(() =>
          rpc(current, {
            cmd: "issue_move",
            reff: row.reff,
            pos: delta < 0 ? { at: "before", reff: target.reff } : { at: "after", reff: target.reff },
          }),
        );
      },

      yankRef: () => {
        const row = selectedRow();
        if (!row) return;
        // The friendly handle if it has one — that is what a human pastes into a
        // branch name or a commit message.
        const ref = row.key_alias ?? row.reff;
        void navigator.clipboard
          .writeText(ref)
          .then(() => setToast(`Copied ${ref}`))
          .catch(() => setError("Clipboard blocked by the browser"));
      },
      moveSelection: (delta) => {
        const list = rowsRef.current;
        if (!list.length) return;
        const i = list.findIndex((r) => r.reff === selRef.current);
        const next = list[Math.max(0, Math.min(list.length - 1, (i < 0 ? 0 : i) + delta))];
        if (next) setSelection(next.reff);
      },
      createIssue: () => setComposing({}),
      createProject: () => setComposingProject(true),
      deleteIssue: (reff) =>
        void guard(async () => {
          if (!current) return;
          try {
            await rpc(current, { cmd: "issue_delete", reff });
          } catch (e) {
            // The engine hands back the CLI's own question rather than us
            // inventing one, so modal and terminal cannot disagree on the stakes.
            if (e instanceof ConfirmRequired) {
              // The engine's own words, in our dialog.
              if (await ask.confirm({ title: e.question, confirmText: "Delete", danger: true })) {
                await rpc(current, { cmd: "issue_delete", reff }, { confirm: true });
              }
              return;
            }
            throw e;
          }
        }),

      restoreIssue: (reff) => {
        if (!current) return;
        // `issue_restore` on a live issue still writes a "restored" event, so
        // refusing here keeps the history honest rather than politely noisy.
        const row = rowsRef.current.find((r) => r.reff === reff);
        if (row && !row.tombstone) return setToast("Not deleted");
        void guard(() => rpc(current, { cmd: "issue_restore", reff }));
      },

      /** Toggle, not set: `i` on an issue you hold puts it down (Linear's `I`
       *  self-assigns; the toggle is what a second press should honestly mean). */
      assignMe: () => {
        const row = selectedRow();
        const me = membersRef.current.find((m) => m.me);
        if (!row || !current || !me) return;
        const add = !row.assignees.includes(me.key);
        void guard(() => rpc(current, { cmd: "assign", reff: row.reff, who: [me.key], add }));
      },

      /** Column top/bottom. Same done-column refusal as `reorder`, same reason. */
      moveTo: (pos) => {
        const row = selectedRow();
        const shownBoard = shownRef.current;
        if (!row || !current || !shownBoard) return;
        const col = shownBoard.columns.find((c) => c.state.id === row.status);
        if (!col || col.state.category === "done") return;
        void guard(() => rpc(current, { cmd: "issue_move", reff: row.reff, pos: { at: pos } }));
      },

      toggleCheck: () => {
        const row = selectedRow();
        if (!row) return;
        setChecked((c) => {
          const next = new Set(c);
          if (!next.delete(row.reff)) next.add(row.reff);
          return next;
        });
      },
      checkAll: () => setChecked(new Set(rowsRef.current.map((r) => r.reff))),
      clearChecks: () => setChecked(new Set()),
      openDisplay: () => setDisplayOpen(true),
      openWorkflow: () => setModal("workflow"),
      openRoles: () => setModal("roles"),
    }),
    [current, guard, loadBoard, loadSpaces, predict, selectedRow],
  );

  const ctx: Ctx = useMemo(
    () => ({
      view,
      spaceId: current,
      readOnly,
      selection,
      checkedCount: checked.size,
      // An open picker owns the keymap exactly as the palette does: `j` in the
      // assignee menu is cmdk's, not the board's.
      overlay: modal !== null || field !== null,
      app: api,
    }),
    [view, current, readOnly, selection, checked, modal, field, api],
  );

  /**
   * A card was dropped: set its status, then place it.
   *
   * **Two requests, and there is no way to make it one.** `issue_edit` carries
   * `status` but no position; `issue_move` carries `project` and `pos` but no
   * status. So a cross-column drag is two commits and two activity rows — the same
   * wrinkle the composer already documents for "file into a non-default column".
   * That is an honest record of what happened (moved, then placed) rather than a
   * fiction, and the alternative is a `Request` variant that does not exist.
   *
   * The **order is load-bearing**. Status first, position second:
   *
   * - Moving *into* a done status removes the doc from `boards[P]`; moving *out of*
   *   one re-inserts it at the top (`replica.rs:858-869`). Doing the placement first
   *   would have that re-insert stomp the position we just asked for.
   * - Dropping into a done column sends **no** `issue_move` at all (`pos` is null):
   *   done columns are rendered by the append rule and ignore the movable list, so
   *   the write would be invisible at best and a lie about ordering at worst.
   *
   * Only status is predicted — `applyOverlay` re-buckets the row into the new
   * column immediately, which is the part that has to feel instant. Position is not
   * a field `Row` carries, so it settles on the doorbell a few milliseconds later.
   */
  const dropCard = useCallback(
    (reff: string, status: string, pos: BoardPos | null) => {
      const id = currentRef.current;
      if (!id) return;
      const row = rowsRef.current.find((r) => r.reff === reff);
      if (!row) return;

      const changingStatus = row.status !== status;
      if (!changingStatus && !pos) return; // dropped where it already was

      const send = async () => {
        if (changingStatus) await rpc(id, { cmd: "issue_edit", reff, status });
        if (pos) await rpc(id, { cmd: "issue_move", reff, pos });
      };

      if (changingStatus) {
        void predict(row.doc_id, "status", status, send);
      } else {
        void guard(send);
      }
    },
    [guard, predict],
  );

  const pending = useKeys(ctx);
  // Width + collapsed state, persisted to localStorage by the library.
  const layout = useDefaultLayout({ id: "lait.layout", panelIds: ["sidebar", "main"] });

  useEffect(() => {
    registry.validate();
  }, []);

  // A prediction whose request neither errored nor rang is stuck: a dropped fetch,
  // a suspended tab.
  //
  // Sweeping is only half the job. Dropping the guess leaves the **pre-write**
  // value on screen with the uncertainty mark removed — the server's stale answer,
  // now presented as confirmed. That is worse than the guess was: at least the
  // guess admitted it was one. So a sweep re-reads.
  //
  // Deps are `[loadBoard]`, which is stable. Keying this on `predicted` tore the
  // interval down and rebuilt it on every prediction, so steady editing or a busy
  // doorbell stream could reset the timer indefinitely and it would never fire —
  // the one thing it exists to do.
  useEffect(() => {
    const t = window.setInterval(() => {
      if (!overlay.current.sweep()) return;
      setPredicted((n) => n + 1);
      void loadBoard(currentRef.current, projectRef.current);
      // The detail pane reads off `revision`, not the board.
      setRevision((r) => r + 1);
    }, PREDICTION_TTL_MS / 2);
    return () => window.clearInterval(t);
  }, [loadBoard]);

  const run = (id: string) => void registry.get(id)?.run(ctx);

  return (
    <TooltipProvider>
    <Group
      orientation="horizontal"
      // Persisted per-user: a sidebar width you set once should survive a reload,
      // and the library already owns that — no state of ours to get wrong.
      {...layout}
      className="flex h-full"
    >
      <Panel
        id="sidebar"
        panelRef={sidebar}
        defaultSize="18%"
        minSize="140px"
        maxSize="32%"
        collapsible
        collapsedSize={0}
        className="bg-raised"
      >
        <Sidebar spaces={spaces} current={current} onPick={api.pickSpace} />
      </Panel>

      {/* A 1px seam with a 7px hit area: thin to look at, big enough to grab. */}
      <Separator className="bg-line data-[state=dragging]:bg-accent hover:bg-accent/60 relative w-px outline-none transition-colors">
        <span className="absolute inset-y-0 -left-[3px] w-[7px]" />
      </Separator>

      <Panel id="main" className="flex min-w-0 flex-col">
        {/*
          Chrome recedes. Linear's header is a breadcrumb and a few ghost icons —
          no bordered CTA competing with the content, no permanently-lit status
          badge. Ours had a segmented control, a primary button, and a `Ctrl K`
          chip all shouting at once; the work is the content, not the toolbar.
        */}
        <header className="border-line flex h-11 shrink-0 items-center gap-1 border-b px-2">
          <IconButton label="Toggle sidebar" chord="⌘B" onClick={() => run("view.sidebar")}>
            <PanelLeft className="size-4" />
          </IconButton>

          {/*
            The project is a *switch*, not a label.

            It read as a title before, which quietly made the client single-project:
            `board` was sent with `project: null` forever, so a space with three
            projects only ever showed whichever one the daemon's default chain
            picked, and the other two were unreachable from the browser. The name was
            never decoration — it was the one control the header was missing.
          */}
          <h1 className="ml-1 flex min-w-0 items-baseline gap-1.5">
            {projects.length > 1 ? (
              <Combobox
                variant="bare"
                label="Project"
                className="font-semibold"
                value={
                  board
                    ? {
                        id: board.project.key,
                        label: board.project.name,
                        swatch: catalogColor(board.project.color),
                      }
                    : null
                }
                options={projects.map((p) => ({
                  id: p.key,
                  label: p.name,
                  swatch: catalogColor(p.color),
                  hint: p.key,
                }))}
                // Straight to the api, not through a command: a command's `run`
                // takes only a `Ctx`, so "pick *this* project" has no way to travel
                // through the registry. Same reason `Sidebar` calls `pickSpace`
                // directly — selection carries an argument, actions don't.
                onPick={api.pickProject}
              />
            ) : (
              <span className="truncate font-semibold">{board?.project.name ?? "lait"}</span>
            )}
            <span className="text-mute shrink-0">/</span>
            <span className="text-dim shrink-0 capitalize">{view}</span>
          </h1>

          <span className="ml-auto flex items-center gap-1">
            {/* Only when it is worth saying. A permanently-lit "live" is noise;
                a silent failure is worse. So: nothing when healthy, a warning
                when not. */}
            {liveness !== "live" && (
              <span
                className="text-warn mr-1 flex items-center gap-1.5 text-xs"
                title={`Doorbell stream: ${liveness}`}
                role="status"
              >
                <span className="bg-warn size-1.5 animate-pulse rounded-full" />
                {liveness}
              </span>
            )}

            <IconButton label="Search commands" chord="⌘K" onClick={() => run("palette.open")}>
              <Search className="size-4" />
            </IconButton>

            {(view === "list" || view === "board") && (
              <>
                <IconButton
                  label="Filter"
                  chord="/"
                  variant={isActive(filter) ? "active" : "ghost"}
                  onClick={() => run("filter.open")}
                >
                  <ListFilter className="size-4" />
                </IconButton>
                <DisplayOptions
                  display={display}
                  view={view}
                  open={displayOpen}
                  onOpenChange={setDisplayOpen}
                  onChange={setDisplay}
                />
              </>
            )}

            {/* A segmented group without a box around it: adjacency does the
                grouping, the active fill does the state. */}
            <span className="mx-1 flex items-center gap-0.5">
              {(
                [
                  ["list", List, "Issues", "G L"],
                  ["board", LayoutGrid, "Board", "G B"],
                  ["inbox", InboxIcon, "Inbox", "G I"],
                ] as const
              ).map(([v, Icon, label, chord]) => (
                <IconButton
                  key={v}
                  label={label}
                  chord={chord}
                  variant={view === v ? "active" : "ghost"}
                  aria-pressed={view === v}
                  onClick={() => run(`go.${v}`)}
                  className="relative"
                >
                  <Icon className="size-4" />
                  {v === "inbox" && unread > 0 && (
                    <span className="bg-accent absolute top-0.5 right-0.5 size-1.5 rounded-full" />
                  )}
                </IconButton>
              ))}
            </span>

            {!readOnly && current && (
              <IconButton label="New issue" chord="C" onClick={() => run("issue.create")}>
                <Plus className="size-4" />
              </IconButton>
            )}
          </span>
        </header>

        {error && (
          <p className="border-line text-danger border-b px-4 py-2 text-sm" role="alert">
            {error}
          </p>
        )}

        {filterOpen && (view === "list" || view === "board") && (
          <FilterBar
            filter={filter}
            labels={labels}
            states={states}
            focusToken={focusToken}
            onChange={setFilter}
            onClose={() => setFilterOpen(false)}
          />
        )}

        <div className="group/list flex min-h-0 flex-1 flex-col">
          {!current ? (
            <p className="text-mute p-8 text-center">Pick a space.</p>
          ) : view === "inbox" ? (
            <Inbox
              spaceId={current}
              revision={revision}
              onError={setError}
              onCountChange={setUnread}
              onOpen={(reff) => {
                api.select(reff);
                setView("list");
              }}
            />
          ) : view === "members" ? (
            <Members
              spaceId={current}
              revision={revision}
              readOnly={readOnly}
              onError={setError}
            />
          ) : view === "activity" ? (
            <Activity
              spaceId={current}
              members={members}
              revision={revision}
              onError={setError}
              onOpen={api.select}
            />
          ) : shown && view === "board" ? (
            <Board
              board={shown}
              members={members}
              selection={selection}
              optimistic={optimistic}
              onSelect={api.select}
              onCreate={(status) => setComposing({ status })}
              onDrop={dropCard}
              readOnly={readOnly}
            />
          ) : shown && view === "list" ? (
            <IssueList
              groups={groups}
              deleted={display.deleted ? deletedRows : []}
              states={states}
              members={members}
              selection={selection}
              checked={checked}
              optimistic={optimistic}
              onSelect={api.select}
              onToggleCheck={(reff) =>
                setChecked((c) => {
                  const next = new Set(c);
                  if (!next.delete(reff)) next.add(reff);
                  return next;
                })
              }
              onOpen={() => setDetail(true)}
              onCreate={(status) => setComposing({ status })}
              readOnly={readOnly}
            />
          ) : (
            <p className="text-mute p-8 text-center">Not built yet.</p>
          )}
        </div>
      </Panel>

      {detail && selection && current && board && (view === "list" || view === "board") && (
        <>
          <Separator className="bg-line data-[state=dragging]:bg-accent hover:bg-accent/60 relative w-px outline-none transition-colors">
            <span className="absolute inset-y-0 -left-[3px] w-[7px]" />
          </Separator>
          <Panel id="detail" defaultSize="30%" minSize="260px" maxSize="50%">
            <IssueDetail
              // Remount on a different issue: a stale draft must not survive into
              // the next one, and `key` says that in one line.
              key={selection}
              spaceId={current}
              reff={selection}
              states={states}
              members={members}
              labels={labels}
              projects={projects}
              readOnly={readOnly}
              // A deleted issue is not on the board at all, so the trash rows
              // are the only place its tombstone can be read from.
              tombstone={deletedRows.some((r) => r.reff === selection)}
              openField={field}
              onOpenField={setField}
              revision={revision}
              onError={setError}
              onDelete={api.deleteIssue}
              onPredict={api.predict}
              onNavigate={api.select}
            />
          </Panel>
        </>
      )}

      {composing && current && board && (
        <NewIssue
          spaceId={current}
          projectKey={board.project.key}
          states={states}
          labels={labels}
          members={members}
          defaultStatus={composing.status}
          onClose={() => setComposing(null)}
          onError={setError}
        />
      )}
      {composingProject && current && (
        <NewProject
          spaceId={current}
          taken={projects.map((p) => p.key.toUpperCase())}
          onClose={() => setComposingProject(false)}
          // Land in what you just made. Creating a project and staying on the old
          // board is the app ignoring the thing you came to do.
          onCreated={(key) => setProject(key)}
          onError={setError}
        />
      )}
      {checked.size > 0 && !readOnly && current && (
        <BulkBar
          count={checked.size}
          states={states}
          labels={labels}
          members={members}
          onStatus={(id) =>
            void bulk((reff) => rpc(current, { cmd: "issue_edit", reff, status: id }))
          }
          onPriority={(id) =>
            void bulk((reff) => rpc(current, { cmd: "issue_edit", reff, priority: id }))
          }
          onLabel={(name) => void bulk((reff) => rpc(current, { cmd: "label", reff, add: [name] }))}
          onAssign={(key) =>
            void bulk((reff) => rpc(current, { cmd: "assign", reff, who: [key], add: true }))
          }
          onDelete={() =>
            void (async () => {
              const n = checked.size;
              // The engine's per-issue question doesn't scale to a set, so the
              // dialog owns the aggregate phrasing and each request then rides
              // with `confirm` — the same consent, asked once.
              const ok = await ask.confirm({
                title: `Delete ${n} ${n === 1 ? "issue" : "issues"}?`,
                body: "Deletion tombstones — they can be restored later.",
                confirmText: "Delete",
                danger: true,
              });
              if (!ok) return;
              await bulk((reff) => rpc(current, { cmd: "issue_delete", reff }, { confirm: true }));
              setChecked(new Set());
            })()
          }
          onClear={() => setChecked(new Set())}
        />
      )}
      <DialogHost />
      {modal === "palette" && <Palette ctx={ctx} onClose={() => setModal(null)} />}
      {modal === "shortcuts" && <Shortcuts ctx={ctx} onClose={() => setModal(null)} />}
      {modal === "workflow" && current && board && (
        <WorkflowDialog
          spaceId={current}
          projectKey={board.project.key}
          onClose={() => setModal(null)}
        />
      )}
      {modal === "roles" && current && (
        <RolesDialog spaceId={current} onClose={() => setModal(null)} />
      )}

      {/* A half-typed sequence must be visible, or `g` reads as a dropped key. */}
      {pending.length > 0 && (
        <div className="border-line-strong bg-raised text-dim shadow-overlay fixed bottom-4 left-4 rounded border px-2 py-1 font-mono text-sm">
          {pending.join(" ")} …
        </div>
      )}
      {toast && (
        <div className="border-line-strong bg-raised shadow-overlay fixed bottom-4 left-1/2 -translate-x-1/2 rounded border px-3 py-1.5 text-sm">
          {toast}
        </div>
      )}
    </Group>
    </TooltipProvider>
  );
}

/**
 * The sidebar toggle is a command like everything else.
 *
 * Contributed here rather than in `commands/` because its `run` needs the panel
 * handle only this component holds — but it still goes through the same door, so
 * it lists in the palette, shows in `?`, and is rebindable. A component with a
 * private `keydown` would be a binding nobody could see or change.
 */
contribute({
  commands: [
    {
      id: "view.sidebar",
      title: "Toggle sidebar",
      group: "View",
      keys: ["mod+b"],
      run: (c) => c.app.toggleSidebar(),
    },
  ],
});
