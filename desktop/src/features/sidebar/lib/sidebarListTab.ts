import * as React from "react";

import { setInboxPanelOpen } from "@/features/home/lib/inboxPanelPreference";

/**
 * Which list the left column shows. Picking a rail tab changes only this
 * list; the main pane keeps what it shows until an item in the list is
 * opened. Persisted on this device like other layout preferences.
 */
export type SidebarListTab =
  | "chats"
  | "people"
  | "activity"
  | "agents"
  | "projects"
  | "pulse";

const STORAGE_KEY = "kuzz.desktop.sidebar-list-tab";
const TABS: readonly SidebarListTab[] = [
  "chats",
  "people",
  "activity",
  "agents",
  "projects",
  "pulse",
];

const listeners = new Set<() => void>();
let current = readStored();

function readStored(): SidebarListTab {
  try {
    const stored = globalThis.localStorage?.getItem(STORAGE_KEY);
    return TABS.includes(stored as SidebarListTab)
      ? (stored as SidebarListTab)
      : "chats";
  } catch {
    return "chats";
  }
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function getSidebarListTab(): SidebarListTab {
  return current;
}

export function setSidebarListTab(tab: SidebarListTab): void {
  // The inbox list is Activity's list; rooms opened from it use its layout.
  setInboxPanelOpen(tab === "activity");
  if (tab === current) return;
  current = tab;
  try {
    globalThis.localStorage?.setItem(STORAGE_KEY, tab);
  } catch {
    // Persistence is best-effort; the in-memory value still applies.
  }
  for (const listener of listeners) listener();
}

export function useSidebarListTab(): SidebarListTab {
  return React.useSyncExternalStore(
    subscribe,
    getSidebarListTab,
    () => "chats",
  );
}

/** The list that belongs to a screen opened by route, if it has one. */
export function listTabForView(view: string): SidebarListTab | null {
  switch (view) {
    case "people":
    case "agents":
    case "projects":
    case "pulse":
      return view;
    case "home":
      return "activity";
    default:
      return null;
  }
}
