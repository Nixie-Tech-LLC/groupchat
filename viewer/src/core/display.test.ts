import { describe, expect, it } from "vitest";

import { DEFAULT_DISPLAY, groupRows } from "./display";
import type { BoardView, Row } from "../types";

const row = (over: Partial<Row> & { reff: string }): Row => ({
  doc_id: `doc_${over.reff}`,
  project_id: "prj_1",
  key_alias: null,
  title: "",
  status: "backlog",
  priority: "none",
  assignee_summary: "",
  assignees: [],
  tombstone: false,
  provisional: false,
  ...over,
});

const board = (rows: Row[]): BoardView => ({
  schema_version: 1,
  project: { id: "prj_1", name: "P", key: "P", color: "blue" },
  columns: [
    {
      state: { id: "backlog", name: "Backlog", category: "backlog", color: "gray" },
      rows: rows.filter((r) => r.status === "backlog"),
    },
    {
      state: { id: "done", name: "Done", category: "done", color: "green" },
      rows: rows.filter((r) => r.status === "done"),
    },
  ],
});

describe("groupRows", () => {
  const rows = [
    row({ reff: "a", title: "zebra", priority: "low", assignees: ["k1"] }),
    row({ reff: "b", title: "apple", priority: "urgent", assignees: ["k2", "k1"] }),
    row({ reff: "c", title: "mango", status: "done" }),
  ];

  it("status grouping is the board's own columns, order untouched", () => {
    const groups = groupRows(board(rows), DEFAULT_DISPLAY);
    expect(groups.map((g) => g.key)).toEqual(["backlog", "done"]);
    expect(groups[0]!.state?.name).toBe("Backlog");
    expect(groups[0]!.rows.map((r) => r.reff)).toEqual(["a", "b"]);
  });

  it("groups by first assignee, one group per issue, unassigned last", () => {
    const groups = groupRows(board(rows), { ...DEFAULT_DISPLAY, group: "assignee" });
    expect(groups.map((g) => g.key)).toEqual(["k1", "k2", "unassigned"]);
    // b has two assignees but appears exactly once (under k2, its first).
    expect(groups.flatMap((g) => g.rows).filter((r) => r.reff === "b")).toHaveLength(1);
    expect(groups[2]!.label).toBe("Unassigned");
  });

  it("groups by priority, highest first, empty tiers dropped", () => {
    const groups = groupRows(board(rows), { ...DEFAULT_DISPLAY, group: "priority" });
    expect(groups.map((g) => g.key)).toEqual(["urgent", "low", "none"]);
  });

  it("orders by priority stably and by title alphabetically", () => {
    const byPriority = groupRows(board(rows), { ...DEFAULT_DISPLAY, group: "none", order: "priority" });
    expect(byPriority[0]!.rows.map((r) => r.reff)).toEqual(["b", "a", "c"]);
    const byTitle = groupRows(board(rows), { ...DEFAULT_DISPLAY, group: "none", order: "title" });
    expect(byTitle[0]!.rows.map((r) => r.reff)).toEqual(["b", "c", "a"]);
  });
});
