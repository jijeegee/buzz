import * as React from "react";

import type { InboxRoomEntry } from "@/features/home/lib/inboxRoomEntry";

/**
 * Whether the inbox list is pulled out beside the chat screen.
 *
 * The inbox is another way into the same chat screen Chats uses: on a channel
 * it is a collapsible list next to the room, hidden while browsing Chats and
 * shown by the sidebar's Inbox button. Persisted in localStorage like other
 * device-level layout preferences.
 */
const STORAGE_KEY = "buzz.desktop.inbox-panel-open";

const listeners = new Set<() => void>();

let isOpen = readStoredOpen();

function readStoredOpen(): boolean {
  try {
    return globalThis.localStorage?.getItem(STORAGE_KEY) === "true";
  } catch {
    return false;
  }
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function getSnapshot(): boolean {
  return isOpen;
}

export function isInboxPanelOpen(): boolean {
  return isOpen;
}

export function setInboxPanelOpen(open: boolean): void {
  if (open === isOpen) return;
  isOpen = open;
  try {
    globalThis.localStorage?.setItem(STORAGE_KEY, String(open));
  } catch {
    // Persistence is best-effort; the in-memory value still applies.
  }
  for (const listener of listeners) {
    listener();
  }
}

export function toggleInboxPanel(): void {
  setInboxPanelOpen(!isOpen);
}

export function useInboxPanelOpen(): boolean {
  return React.useSyncExternalStore(subscribe, getSnapshot, () => false);
}

/**
 * The latest chat-room entry made from the inbox. Each entry gets a new nonce
 * so the mounted chat screen lands exactly where the row points, even when the
 * same row is clicked again: "click a row, you are there".
 */
export type InboxRoomLanding = InboxRoomEntry & { nonce: number };

const landingListeners = new Set<() => void>();

let landing: InboxRoomLanding | null = null;

export function recordInboxRoomLanding(
  entry: Omit<InboxRoomLanding, "nonce">,
): void {
  landing = { ...entry, nonce: (landing?.nonce ?? 0) + 1 };
  for (const listener of landingListeners) {
    listener();
  }
}

function subscribeLanding(listener: () => void): () => void {
  landingListeners.add(listener);
  return () => {
    landingListeners.delete(listener);
  };
}

export function useInboxRoomLanding(): InboxRoomLanding | null {
  return React.useSyncExternalStore(
    subscribeLanding,
    () => landing,
    () => null,
  );
}
