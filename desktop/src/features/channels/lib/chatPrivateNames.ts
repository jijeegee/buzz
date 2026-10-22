import * as React from "react";

/**
 * Private chat names ("Rename for me"): one map of channel id -> name per
 * account, synced across that account's devices as kind:30078
 * `d=channel-names` (NIP-44 to self, JSON `{"<channel_id>": "name"}`) and
 * invisible to everyone else. An empty or missing entry means "no private
 * name". The map lives in this module so non-React title paths (desktop
 * notifications, reminders) read the same names the UI shows.
 */
export type ChatPrivateNames = ReadonlyMap<string, string>;

/** Longest private name kept; longer input is cut. */
export const MAX_PRIVATE_CHAT_NAME_CHARS = 80;
/** Most entries kept in one blob (the newest edits win when trimming). */
export const MAX_PRIVATE_CHAT_NAMES = 500;

const EMPTY: ChatPrivateNames = new Map();
let current: ChatPrivateNames = EMPTY;
const listeners = new Set<() => void>();

export function getChatPrivateNames(): ChatPrivateNames {
  return current;
}

export function getChatPrivateName(channelId: string): string | null {
  return current.get(channelId) ?? null;
}

export function setChatPrivateNames(next: ChatPrivateNames): void {
  if (sameNames(current, next)) return;
  current = next;
  for (const listener of listeners) listener();
}

export function subscribeChatPrivateNames(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** Re-renders when any private name changes. */
export function useChatPrivateNames(): ChatPrivateNames {
  return React.useSyncExternalStore(
    subscribeChatPrivateNames,
    getChatPrivateNames,
    getChatPrivateNames,
  );
}

function sameNames(left: ChatPrivateNames, right: ChatPrivateNames) {
  if (left.size !== right.size) return false;
  for (const [id, name] of left) if (right.get(id) !== name) return false;
  return true;
}

export function normalizePrivateChatName(name: string | null | undefined) {
  const trimmed = (name ?? "").trim();
  return Array.from(trimmed).slice(0, MAX_PRIVATE_CHAT_NAME_CHARS).join("");
}

/** Parse the decrypted blob; anything that is not a string map is dropped. */
export function parsePrivateNamesPayload(json: unknown): Map<string, string> {
  const out = new Map<string, string>();
  if (typeof json !== "object" || json === null || Array.isArray(json)) {
    return out;
  }
  for (const [id, value] of Object.entries(json as Record<string, unknown>)) {
    if (typeof value !== "string") continue;
    const name = normalizePrivateChatName(value);
    if (id && name) out.set(id, name);
  }
  return out;
}

export function serializePrivateNames(names: ChatPrivateNames): string {
  const entries = [...names].slice(-MAX_PRIVATE_CHAT_NAMES);
  return JSON.stringify(Object.fromEntries(entries));
}

/** Set (non-empty) or clear (empty / null) one chat's private name. */
export function withPrivateName(
  names: ChatPrivateNames,
  channelId: string,
  name: string | null,
): Map<string, string> {
  const next = new Map(names);
  next.delete(channelId);
  const normalized = normalizePrivateChatName(name);
  // Re-inserted last so trimming to the cap keeps the newest edit.
  if (normalized) next.set(channelId, normalized);
  return next;
}

function cacheKey(pubkey: string) {
  return `kuzz-chat-private-names.v1:${pubkey.toLowerCase()}`;
}

export function readCachedPrivateNames(pubkey: string): Map<string, string> {
  try {
    const raw = window.localStorage.getItem(cacheKey(pubkey));
    return raw ? parsePrivateNamesPayload(JSON.parse(raw)) : new Map();
  } catch {
    return new Map();
  }
}

export function writeCachedPrivateNames(
  pubkey: string,
  names: ChatPrivateNames,
): void {
  try {
    window.localStorage.setItem(cacheKey(pubkey), serializePrivateNames(names));
  } catch {
    // The relay copy stays authoritative; the cache only speeds up startup.
  }
}
