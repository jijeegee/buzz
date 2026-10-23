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
// Tabs a reload may return to. Activity and Pulse have no rail item for
// now, so a stored one falls back to Chats.
const TABS: readonly SidebarListTab[] = [
  "chats",
  "people",
  "agents",
  "projects",
];

/**
 * The app was started without a route (no hash), as on a fresh launch. A
 * reload keeps its hash, so it returns to the screen it was on.
 */
export const STARTED_WITHOUT_ROUTE =
  typeof window === "undefined" ||
  window.location.hash === "" ||
  window.location.hash === "#";

const listeners = new Set<() => void>();
// A fresh launch always opens on Chats; a reload keeps the list it was on.
let current: SidebarListTab = STARTED_WITHOUT_ROUTE ? "chats" : readStored();
if (STARTED_WITHOUT_ROUTE) setInboxPanelOpen(false);

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
    // Home is Chats; the Inbox is hidden.
    case "home":
      return "chats";
    default:
      return null;
  }
}
