import * as React from "react";

import { type WatchedUnread, watchedUnreadStore } from "./store";

/** Unread state of every watched account, keyed `origin|pubkey`. */
export function useWatchedUnread(): ReadonlyMap<string, WatchedUnread> {
  return React.useSyncExternalStore(
    watchedUnreadStore.subscribe,
    watchedUnreadStore.snapshot,
  );
}

/** Whether any account other than the open one has something unread. */
export function anyWatchedUnread(
  unread: ReadonlyMap<string, WatchedUnread>,
): boolean {
  for (const entry of unread.values()) {
    if (entry.hasUnread || entry.mentionCount > 0) return true;
  }
  return false;
}
