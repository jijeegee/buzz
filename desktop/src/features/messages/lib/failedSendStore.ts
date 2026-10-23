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

/** Keys saved by this app session; anything else on disk is from before. */
const sessionKeys = new Set<string>();

function storeKey(): string | null {
  const scope = getDraftStorePersistScope();
  return scope ? `${STORE_KEY_PREFIX}:${scope}` : null;
}

function readAll(): StoredSend[] {
  const key = storeKey();
  if (!key) return [];
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(key) ?? "[]");
    return Array.isArray(parsed) ? (parsed as StoredSend[]) : [];
  } catch {
    return [];
  }
}

function writeAll(entries: StoredSend[]) {
  const key = storeKey();
  if (!key) return;
  if (entries.length === 0) {
    localStorage.removeItem(key);
    return;
  }
  setLocalStorageItemWithRecovery(key, JSON.stringify(entries));
}

export function saveStoredSend(entry: StoredSend) {
  sessionKeys.add(entry.localKey);
  writeAll([
    ...readAll().filter((stored) => stored.localKey !== entry.localKey),
    entry,
  ]);
}

export function removeStoredSend(localKey: string) {
  sessionKeys.delete(localKey);
  const entries = readAll();
  const next = entries.filter((stored) => stored.localKey !== localKey);
  if (next.length !== entries.length) writeAll(next);
}

/** Sends stored by an earlier app session for this channel. */
export function earlierStoredSends(channelId: string): StoredSend[] {
  return readAll().filter(
    (stored) =>
      stored.channelId === channelId && !sessionKeys.has(stored.localKey),
  );
}

export function resetStoredSendSession() {
  sessionKeys.clear();
}
