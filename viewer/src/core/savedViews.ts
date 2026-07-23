import type { DisplayState } from "./display";
import type { FilterState } from "./filter";

const KEY = "lait.saved-views";

export interface SavedView {
  id: string;
  name: string;
  filter: FilterState;
  display: DisplayState;
}

export function loadSavedViews(space: string, project: string): SavedView[] {
  try {
    const all = JSON.parse(localStorage.getItem(KEY) ?? "{}") as Record<string, SavedView[]>;
    return all[scope(space, project)] ?? [];
  } catch {
    return [];
  }
}

export function saveView(space: string, project: string, view: SavedView): SavedView[] {
  const all = readAll();
  const scoped = all[scope(space, project)] ?? [];
  const next = [view, ...scoped.filter((item) => item.id !== view.id)];
  all[scope(space, project)] = next;
  writeAll(all);
  return next;
}

export function removeView(space: string, project: string, id: string): SavedView[] {
  const all = readAll();
  const next = (all[scope(space, project)] ?? []).filter((view) => view.id !== id);
  all[scope(space, project)] = next;
  writeAll(all);
  return next;
}

function scope(space: string, project: string): string {
  return `${space}:${project}`;
}

function readAll(): Record<string, SavedView[]> {
  try {
    return JSON.parse(localStorage.getItem(KEY) ?? "{}") as Record<string, SavedView[]>;
  } catch {
    return {};
  }
}

function writeAll(all: Record<string, SavedView[]>): void {
  try {
    localStorage.setItem(KEY, JSON.stringify(all));
  } catch {
    // Views remain usable for the session even when persistence is unavailable.
  }
}
