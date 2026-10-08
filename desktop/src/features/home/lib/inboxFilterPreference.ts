import * as React from "react";

import { type InboxFilter, parseInboxFilter } from "@/features/home/lib/inbox";
import { getStorageItem, setStorageItem } from "@/shared/lib/safeStorage";

/**
 * The chosen inbox filter: a device-level preference that survives restarts,
 * shared by the inbox list and the feed layer (which loads extra channel
 * traffic only for the Channels + Threads view).
 */
const STORAGE_KEY = "buzz.desktop.inbox-filter";

const listeners = new Set<() => void>();

let current: InboxFilter = parseInboxFilter(getStorageItem(STORAGE_KEY));

export function setInboxFilter(filter: InboxFilter): void {
  if (filter === current) return;
  current = filter;
  setStorageItem(STORAGE_KEY, filter);
  for (const listener of listeners) {
    listener();
  }
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function useInboxFilter(): InboxFilter {
  return React.useSyncExternalStore(
    subscribe,
    () => current,
    () => "all",
  );
}
