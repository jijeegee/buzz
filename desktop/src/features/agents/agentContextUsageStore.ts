import * as React from "react";

import {
  getAgentObserverSnapshot,
  isObserverEventAfter,
  subscribeAgentObserverStore,
  type AgentObserverStoreUpdate,
} from "@/features/agents/observerRelayStore";
import { normalizePubkey } from "@/shared/lib/pubkey";
import { getStorageItem, setStorageItem } from "@/shared/lib/safeStorage";
import type { ObserverEvent } from "./ui/agentSessionTypes";

/**
 * Latest context-window reading for one agent session scope.
 *
 * A scope is (agent, channel, thread root). `threadRootEventId` is null for a
 * whole-conversation scope (DMs, and every channel event under the harness's
 * Channel session policy).
 */
export type AgentContextUsage = {
  /** Normalized agent pubkey. */
  agentPubkey: string;
  channelId: string;
  threadRootEventId: string | null;
  sessionId: string | null;
  used: number;
  size: number;
  /** Whether the agent runtime advertised a compaction command. */
  compactSupported: boolean;
  /** Reading time in epoch ms, from the observer event timestamp. */
  updatedAt: number;
  /** Observer ordering key of the event that produced this reading. */
  timestamp: string;
  seq: number;
};

/** Observer event kind carrying a per-session context reading. */
export const CONTEXT_USAGE_EVENT_KIND = "context_usage";
/** Upper bound on retained scopes; the least recently updated are evicted. */
export const MAX_CONTEXT_USAGE_ENTRIES = 400;
const STORAGE_PREFIX = "buzz-agent-context-usage.v1";

const entries = new Map<string, AgentContextUsage>();
const listeners = new Set<() => void>();
const cachedAgentLists = new Map<string, AgentContextUsage[]>();
const EMPTY_USAGES: AgentContextUsage[] = [];

// The identity whose observer stream produced the readings. Persistence is
// keyed by it so a different signed-in identity never sees another owner's
// session readings. Null until the bridge resolves the identity; readings that
// arrive before then stay in memory and merge into that owner's store.
let ownerScope: string | null = null;

function scopeKey(
  agentPubkey: string,
  channelId: string,
  threadRootEventId: string | null,
): string {
  return `${normalizePubkey(agentPubkey)}\u0000${channelId}\u0000${threadRootEventId ?? ""}`;
}

function storageKey(owner: string): string {
  return `${STORAGE_PREFIX}:${owner}`;
}

function notifyListeners() {
  cachedAgentLists.clear();
  for (const listener of listeners) {
    listener();
  }
}

function isNonNegativeFinite(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value) && value >= 0;
}

function normalizeThreadRoot(value: unknown): string | null {
  return typeof value === "string" && value.length > 0
    ? value.toLowerCase()
    : null;
}

/**
 * Parse one observer event into a context reading, or null when it is not a
 * well-formed `context_usage` event with a channel and a positive window.
 */
export function parseContextUsageEvent(
  agentPubkey: string,
  event: ObserverEvent,
): AgentContextUsage | null {
  if (event.kind !== CONTEXT_USAGE_EVENT_KIND || !event.channelId) {
    return null;
  }
  const payload = event.payload as Record<string, unknown> | null;
  if (typeof payload !== "object" || payload === null) {
    return null;
  }
  const { used, size } = payload;
  if (!isNonNegativeFinite(used) || !isNonNegativeFinite(size) || size <= 0) {
    return null;
  }
  const parsedAt = Date.parse(event.timestamp);
  const sessionId =
    typeof payload.sessionId === "string"
      ? payload.sessionId
      : (event.sessionId ?? null);
  return {
    agentPubkey: normalizePubkey(agentPubkey),
    channelId: event.channelId,
    threadRootEventId: normalizeThreadRoot(payload.threadRootEventId),
    sessionId,
    used,
    size,
    compactSupported: payload.compactSupported === true,
    updatedAt: Number.isFinite(parsedAt) ? parsedAt : Date.now(),
    timestamp: event.timestamp,
    seq: event.seq,
  };
}

/** Keep `reading` only if it is strictly newer than the stored one. */
function admitReading(reading: AgentContextUsage): boolean {
  const key = scopeKey(
    reading.agentPubkey,
    reading.channelId,
    reading.threadRootEventId,
  );
  const existing = entries.get(key);
  if (existing && !isObserverEventAfter(reading, existing)) {
    return false;
  }
  entries.set(key, reading);
  return true;
}

function enforceBound() {
  if (entries.size <= MAX_CONTEXT_USAGE_ENTRIES) return;
  const oldestFirst = [...entries.entries()].sort(
    (left, right) => left[1].updatedAt - right[1].updatedAt,
  );
  for (const [key] of oldestFirst.slice(
    0,
    entries.size - MAX_CONTEXT_USAGE_ENTRIES,
  )) {
    entries.delete(key);
  }
}

function persist() {
  if (!ownerScope) return;
  setStorageItem(
    storageKey(ownerScope),
    JSON.stringify({ entries: [...entries.values()] }),
  );
}

function isStoredReading(value: unknown): value is AgentContextUsage {
  if (typeof value !== "object" || value === null) return false;
  const record = value as Record<string, unknown>;
  return (
    typeof record.agentPubkey === "string" &&
    typeof record.channelId === "string" &&
    (record.threadRootEventId === null ||
      typeof record.threadRootEventId === "string") &&
    (record.sessionId === null || typeof record.sessionId === "string") &&
    isNonNegativeFinite(record.used) &&
    isNonNegativeFinite(record.size) &&
    (record.size as number) > 0 &&
    typeof record.compactSupported === "boolean" &&
    isNonNegativeFinite(record.updatedAt) &&
    typeof record.timestamp === "string" &&
    typeof record.seq === "number"
  );
}

function readPersisted(owner: string): AgentContextUsage[] {
  const raw = getStorageItem(storageKey(owner));
  if (!raw) return [];
  try {
    const parsed = JSON.parse(raw) as { entries?: unknown };
    return Array.isArray(parsed?.entries)
      ? parsed.entries.filter(isStoredReading)
      : [];
  } catch {
    return [];
  }
}

/**
 * Fold observer events for one agent into the store. Newest reading per scope
 * wins by observer ordering (timestamp, then seq), so replays and late frames
 * never regress a scope. Returns true when any scope changed.
 */
export function ingestContextUsageEvents(
  agentPubkey: string,
  events: readonly ObserverEvent[],
): boolean {
  let changed = false;
  for (const event of events) {
    const reading = parseContextUsageEvent(agentPubkey, event);
    if (reading && admitReading(reading)) {
      changed = true;
    }
  }
  if (!changed) return false;
  enforceBound();
  persist();
  notifyListeners();
  return true;
}

/**
 * Bind persistence to the identity whose observer stream feeds the store.
 * Switching from one owner to another replaces the readings; resolving the
 * first owner merges any readings that arrived before identity resolved.
 */
export function setContextUsageOwnerScope(owner: string | null | undefined) {
  const next = owner ? normalizePubkey(owner) : null;
  if (next === ownerScope) return;
  const previous = ownerScope;
  ownerScope = next;
  if (previous !== null) {
    entries.clear();
  }
  if (next !== null) {
    for (const reading of readPersisted(next)) {
      admitReading(reading);
    }
    enforceBound();
    persist();
  }
  notifyListeners();
}

export function subscribeAgentContextUsage(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** Exact-scope lookup. */
export function getAgentContextUsage(
  agentPubkey: string | null | undefined,
  channelId: string | null | undefined,
  threadRootEventId: string | null,
): AgentContextUsage | null {
  if (!agentPubkey || !channelId) return null;
  return (
    entries.get(
      scopeKey(agentPubkey, channelId, normalizeThreadRoot(threadRootEventId)),
    ) ?? null
  );
}

/**
 * Resolve the reading for a chat message by an agent: the thread scope rooted
 * at `threadRootCandidate` (the message's thread root, or the message itself
 * when top-level) first, else the channel's whole-conversation scope (DMs and
 * Channel-policy agents). Null when neither scope has a reading.
 */
export function resolveMessageContextUsage(
  agentPubkey: string | null | undefined,
  channelId: string | null | undefined,
  threadRootCandidate: string | null | undefined,
): AgentContextUsage | null {
  return (
    (threadRootCandidate
      ? getAgentContextUsage(agentPubkey, channelId, threadRootCandidate)
      : null) ?? getAgentContextUsage(agentPubkey, channelId, null)
  );
}

/** Every retained scope for one agent, most recently updated first. */
export function getAgentContextUsages(
  agentPubkey: string | null | undefined,
): AgentContextUsage[] {
  if (!agentPubkey) return EMPTY_USAGES;
  const key = normalizePubkey(agentPubkey);
  const cached = cachedAgentLists.get(key);
  if (cached) return cached;
  const result: AgentContextUsage[] = [];
  for (const reading of entries.values()) {
    if (reading.agentPubkey === key) result.push(reading);
  }
  result.sort((left, right) => right.updatedAt - left.updatedAt);
  const stable = result.length === 0 ? EMPTY_USAGES : result;
  cachedAgentLists.set(key, stable);
  return stable;
}

export type ChannelSessionGroup = {
  /** Whole-conversation scope reading, when one exists. */
  conversation: AgentContextUsage | null;
  /** Thread-scoped readings, most recently updated first. */
  threads: AgentContextUsage[];
};

/** Group one agent's readings (already newest-first) by channel. */
export function groupContextUsagesByChannel(
  usages: readonly AgentContextUsage[],
): Map<string, ChannelSessionGroup> {
  const groups = new Map<string, ChannelSessionGroup>();
  for (const reading of usages) {
    let group = groups.get(reading.channelId);
    if (!group) {
      group = { conversation: null, threads: [] };
      groups.set(reading.channelId, group);
    }
    if (reading.threadRootEventId === null) {
      group.conversation = reading;
    } else {
      group.threads.push(reading);
    }
  }
  return groups;
}

export function useResolvedAgentContextUsage(
  agentPubkey: string | null | undefined,
  channelId: string | null | undefined,
  threadRootCandidate: string | null | undefined,
): AgentContextUsage | null {
  const getSnapshot = React.useCallback(
    () =>
      resolveMessageContextUsage(agentPubkey, channelId, threadRootCandidate),
    [agentPubkey, channelId, threadRootCandidate],
  );
  return React.useSyncExternalStore(subscribeAgentContextUsage, getSnapshot);
}

export function useAgentContextUsages(
  agentPubkey: string | null | undefined,
): AgentContextUsage[] {
  const getSnapshot = React.useCallback(
    () => getAgentContextUsages(agentPubkey),
    [agentPubkey],
  );
  return React.useSyncExternalStore(subscribeAgentContextUsage, getSnapshot);
}

/** Fold every retained observer journal into the store (bridge mount). */
export function syncAgentContextUsageFromObserver(
  agents: readonly { pubkey: string }[],
) {
  for (const agent of agents) {
    ingestContextUsageEvents(
      agent.pubkey,
      getAgentObserverSnapshot(agent.pubkey).events,
    );
  }
}

/** Steady-state listener: only newly admitted observer events are folded. */
export function contextUsageObserverListener(
  update?: AgentObserverStoreUpdate,
) {
  if (!update) return;
  ingestContextUsageEvents(update.agentPubkey, update.events);
}

/**
 * App-level bridge, mounted beside the other observer ingestion bridges so the
 * gauge receives readings on every screen.
 */
export function useAgentContextUsageBridge(
  agents: readonly { pubkey: string }[],
  ownerPubkey: string | null | undefined,
) {
  React.useEffect(() => {
    setContextUsageOwnerScope(ownerPubkey);
  }, [ownerPubkey]);

  React.useEffect(() => {
    syncAgentContextUsageFromObserver(agents);
    return subscribeAgentObserverStore(contextUsageObserverListener);
  }, [agents]);
}

/** Test-only: clear in-memory state and the owner binding. */
export function _testResetAgentContextUsageStore() {
  entries.clear();
  ownerScope = null;
  notifyListeners();
}
