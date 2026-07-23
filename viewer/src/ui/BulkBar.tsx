import { Trash2, X } from "lucide-react";

import type { LabelDto, MemberDto, WorkflowState } from "../types";
import { PRIORITY_ORDER } from "../types";
import { Avatar, memberName } from "./Avatar";
import { catalogColor } from "./colors";
import { PriorityIcon, StatusIcon } from "./icons";
import { Combobox } from "./Picker";
import { IconButton, Kbd } from "./primitives";

/**
 * The bulk-action bar — appears while any issue carries a check (`x`), floats
 * over the list like Linear's, and vanishes with the last check.
 *
 * Every action here is N ordinary `Request`s, one per checked issue, applied in
 * sequence. That is deliberate: the engine's transaction unit is one intent on
 * one issue, so "set 12 issues to Done" *is* twelve commits and twelve activity
 * rows, and pretending otherwise would be a fiction the history couldn't back.
 * The bar is a multiplier on verbs that already exist, never a new verb.
 */
export function BulkBar({
  count,
  progress,
  states,
  labels,
  members,
  onStatus,
  onPriority,
  onLabel,
  onAssign,
  onDelete,
  onClear,
}: {
  count: number;
  progress: { done: number; total: number; failed?: string } | null;
  states: WorkflowState[];
  labels: LabelDto[];
  members: MemberDto[];
  onStatus: (id: string) => void;
  onPriority: (id: string) => void;
  onLabel: (name: string) => void;
  onAssign: (key: string) => void;
  onDelete: () => void;
  onClear: () => void;
}) {
  return (
    <div className="border-line-strong bg-raised shadow-overlay fixed bottom-4 left-1/2 z-40 flex -translate-x-1/2 items-center gap-2 rounded-lg border px-3 py-1.5">
      <span className="text-sm font-medium tabular-nums">{count} selected</span>
      {progress && (
        <span className={progress.failed ? "text-danger text-xs" : "text-mute text-xs"} role="status" aria-live="polite">
          {progress.failed ? `Stopped at ${progress.failed}` : `${progress.done}/${progress.total} complete`}
        </span>
      )}
      <span className="bg-line mx-1 h-4 w-px" />

      <Combobox
        label="Status"
        value={null}
        placeholder="Status"
        options={states.map((s) => ({
          id: s.id,
          label: s.name,
          icon: <StatusIcon category={s.category} color={catalogColor(s.color)} />,
        }))}
        onPick={onStatus}
      />
      <Combobox
        label="Priority"
        value={null}
        placeholder="Priority"
        className="capitalize"
        options={[...PRIORITY_ORDER].reverse().map((p) => ({
          id: p,
          label: p,
          icon: <PriorityIcon priority={p} />,
        }))}
        onPick={onPriority}
      />
      <Combobox
        label="Add label"
        value={null}
        placeholder="Label"
        emptyText={labels.length ? "No matches" : "No labels yet"}
        options={labels.map((l) => ({
          id: l.name,
          label: l.name,
          swatch: catalogColor(l.color),
        }))}
        onPick={onLabel}
        onCreate={onLabel}
      />
      <Combobox
        label="Assign"
        value={null}
        placeholder="Assign"
        emptyText={members.length ? "No matches" : "No members yet"}
        options={members.map((m) => ({
          id: m.key,
          label: memberName(m.key, m),
          icon: <Avatar deviceKey={m.key} alias={m.alias} me={m.me} size="sm" />,
          hint: m.key.slice(0, 6),
          keywords: [m.key, m.alias],
        }))}
        onPick={onAssign}
      />

      <IconButton label="Delete selected" variant="danger" onClick={onDelete}>
        <Trash2 className="size-3.5" />
      </IconButton>

      <span className="bg-line mx-1 h-4 w-px" />
      <IconButton label="Clear selection" chord="Esc" onClick={onClear}>
        <X className="size-3.5" />
      </IconButton>
      <span className="text-mute flex items-center gap-1 text-2xs">
        <Kbd>x</Kbd> toggles
      </span>
    </div>
  );
}
