import * as React from "react";

/**
 * What the owner has seen of an agent's conversations, on this device: per
 * chat or thread (read-state context id) the newest message time looked at.
 * The first look at an agent's list sets a baseline, so older chats start
 * without dots.
 */
type SeenRecord = {
  baseline: number;
  seen: Record<string, number>;
};

/** Contexts kept per agent; the oldest drop first. */
const MAX_SEEN_CONTEXTS = 500;
const STORAGE_PREFIX = "kuzz.ownerView.seen";

const listeners = new Set<() => void>();
let version = 0;

function storageKey(ownerPubkey: string, agentPubkey: string) {
  return `${STORAGE_PREFIX}.${ownerPubkey.toLowerCase()}.${agentPubkey.toLowerCase()}`;
}

function readRecord(key: string): SeenRecord | null {
  try {
    const raw = window.localStorage.getItem(key);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as Partial<SeenRecord>;
    if (typeof parsed.baseline !== "number") return null;
    return {
      baseline: parsed.baseline,
      seen: parsed.seen && typeof parsed.seen === "object" ? parsed.seen : {},
    };
  } catch {
    return null;
  }
}

function writeRecord(key: string, record: SeenRecord) {
  const entries = Object.entries(record.seen);
  if (entries.length > MAX_SEEN_CONTEXTS) {
    entries.sort((left, right) => right[1] - left[1]);
    record.seen = Object.fromEntries(entries.slice(0, MAX_SEEN_CONTEXTS));
  }
  try {
    window.localStorage.setItem(key, JSON.stringify(record));
  } catch {
    // Storage full or unavailable: dots just come back next time.
  }
  version += 1;
  for (const listener of listeners) listener();
}

/** Start the baseline on the first look at this agent's list. */
export function ensureOwnerViewBaseline(
  ownerPubkey: string,
  agentPubkey: string,
  nowSeconds = Math.floor(Date.now() / 1000),
) {
  const key = storageKey(ownerPubkey, agentPubkey);
  if (readRecord(key)) return;
  writeRecord(key, { baseline: nowSeconds, seen: {} });
}

/** The owner looked at this chat or thread up to `createdAt`. */
export function markOwnerViewSeen(
  ownerPubkey: string,
  agentPubkey: string,
  contextId: string,
  createdAt: number,
) {
  const key = storageKey(ownerPubkey, agentPubkey);
  const record = readRecord(key) ?? { baseline: 0, seen: {} };
  if ((record.seen[contextId] ?? 0) >= createdAt) return;
  record.seen[contextId] = createdAt;
  writeRecord(key, record);
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function getVersion() {
  return version;
}

/** `seenAt(contextId)` for one agent; re-renders when anything is marked. */
export function useOwnerViewSeen(
  ownerPubkey: string | undefined,
  agentPubkey: string,
): (contextId: string) => number {
  const current = React.useSyncExternalStore(subscribe, getVersion);
  return React.useMemo(() => {
    void current;
    if (!ownerPubkey) return () => Number.POSITIVE_INFINITY;
    const record = readRecord(storageKey(ownerPubkey, agentPubkey));
    if (!record) return () => Number.POSITIVE_INFINITY;
    return (contextId: string) =>
      Math.max(record.baseline, record.seen[contextId] ?? 0);
  }, [agentPubkey, current, ownerPubkey]);
}
