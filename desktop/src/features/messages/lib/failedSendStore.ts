import type { RelayEvent } from "@/shared/api/types";
import { setLocalStorageItemWithRecovery } from "@/shared/lib/localStorageQuota";
import { getDraftStorePersistScope } from "./useDrafts";

/**
 * Own messages handed to the relay but not yet acknowledged, kept on disk so a
 * send that fails (or an app that quits mid-send) keeps its text: after a
 * restart the row comes back marked failed with Retry / Delete, as it was.
 * Scoped per community relay and signing account, like drafts.
 */
export type StoredSend = {
  localKey: string;
  channelId: string;
  /** Thread root for a reply, so the row can return to that thread too. */
  rootId: string | null;
  /** The row as drawn: its time and tags place it where it was. */
  event: RelayEvent;
  /** What Retry sends again. */
  send: {
    content: string;
    mentionPubkeys: string[];
    mediaTags: string[][];
    parentEventId: string | null;
    sentFromThreadRootId: string | null;
    sentFromThreadRootExcerpt: string | null;
    forceRest: boolean;
  };
};

const STORE_KEY_PREFIX = "kuzz-failed-sends.v1";
/** Unsent messages older than this are let go, like an abandoned draft. */
const MAX_AGE_SECONDS = 7 * 24 * 60 * 60;
/** Newest unsent messages kept per account. */
const MAX_ENTRIES = 50;

/**
 * Where each send of this app session was saved. Removal uses it, so a send
 * that settles after a community or account switch still clears its own
 * entry rather than reading whatever scope is current.
 */
const savedKeys = new Map<string, string>();
/** Sends awaiting the relay; never drawn as failed while still in flight. */
const inFlightKeys = new Set<string>();

function storeKey(): string | null {
  const scope = getDraftStorePersistScope();
  return scope ? `${STORE_KEY_PREFIX}:${scope}` : null;
}

function readAll(key: string | null): StoredSend[] {
  if (!key) return [];
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(key) ?? "[]");
    if (!Array.isArray(parsed)) return [];
    const oldest = Math.floor(Date.now() / 1000) - MAX_AGE_SECONDS;
    return (parsed as StoredSend[]).filter(
      (stored) => stored.event?.created_at >= oldest,
    );
  } catch {
    return [];
  }
}

function writeAll(key: string, entries: StoredSend[]) {
  if (entries.length === 0) {
    localStorage.removeItem(key);
    return;
  }
  setLocalStorageItemWithRecovery(key, JSON.stringify(entries));
}

/** Save a send as it leaves; it counts as in flight until settled. */
export function saveStoredSend(entry: StoredSend) {
  const key = storeKey();
  if (!key) return;
  savedKeys.set(entry.localKey, key);
  inFlightKeys.add(entry.localKey);
  writeAll(
    key,
    [
      ...readAll(key).filter((stored) => stored.localKey !== entry.localKey),
      entry,
    ].slice(-MAX_ENTRIES),
  );
}

/** Mark a send as on its way again (Retry), before its mutation starts. */
export function markStoredSendInFlight(localKey: string) {
  inFlightKeys.add(localKey);
}

/** The send failed: keep it stored, but it is no longer in flight. */
export function settleStoredSend(localKey: string) {
  inFlightKeys.delete(localKey);
}

/** The send was delivered or deleted: forget it. */
export function removeStoredSend(localKey: string) {
  inFlightKeys.delete(localKey);
  const key = savedKeys.get(localKey) ?? storeKey();
  savedKeys.delete(localKey);
  if (!key) return;
  const entries = readAll(key);
  const next = entries.filter((stored) => stored.localKey !== localKey);
  if (next.length !== entries.length) writeAll(key, next);
}

/** Stored sends for a channel in the current scope, in flight or not. */
export function storedSends(channelId: string): StoredSend[] {
  return readAll(storeKey()).filter((stored) => stored.channelId === channelId);
}

export function isStoredSendInFlight(localKey: string): boolean {
  return inFlightKeys.has(localKey);
}

/** Forget in-memory send state, as a fresh app start would (tests). */
export function resetStoredSendSession() {
  savedKeys.clear();
  inFlightKeys.clear();
}
