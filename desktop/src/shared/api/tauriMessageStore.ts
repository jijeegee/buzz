import { invokeTauri } from "@/shared/api/tauri";
import type { RelayEvent } from "@/shared/api/types";

/** One account on one server; each scope is its own local database. */
export type MessageStoreScope = { pubkey: string; relayUrl: string };
export type MessageStoreCursor = { createdAt: number; eventId: string };

/** A room page from the local store, newest first. Never crosses a gap. */
export type MessageStoreWindowPage = {
  startCursor: MessageStoreCursor | null;
  rows: RelayEvent[];
  /** Reactions, edits and deletes targeting `rows`, oldest first. */
  aux: RelayEvent[];
  /** Newest thread summary (39005) per row that has one. */
  summaries: RelayEvent[];
  nextCursor: MessageStoreCursor | null;
  hasMore: boolean;
  /** Stopped at the edge of local coverage: ask the server below `nextCursor`. */
  gap: boolean;
};

export type MessageStoreAroundPage = {
  rows: RelayEvent[];
  aux: RelayEvent[];
  summaries: RelayEvent[];
  hasOlder: boolean;
  hasNewer: boolean;
  gapBelow: boolean;
  gapAbove: boolean;
};

export type MessageStoreThreadPage = {
  rows: RelayEvent[];
  aux: RelayEvent[];
  summaries: RelayEvent[];
  nextCursor: MessageStoreCursor | null;
  hasMore: boolean;
  coveredThrough: number | null;
  liveCovered: boolean;
};

export type MessageStoreRange = {
  class: "top" | "all";
  loAt: number;
  loId: string;
  loIsStart: boolean;
  hiAt: number;
};

export type MessageStoreRoomCoverage = {
  channelId: string;
  ranges: MessageStoreRange[];
  liveCovered: boolean;
  provenThrough: number | null;
  newestLocalAt: number | null;
  gapBelow: boolean;
};

export type MessageStoreRoomHead = {
  channelId: string;
  page: MessageStoreWindowPage;
};

export function loadMessageStoreHeads(
  scope: MessageStoreScope,
  rooms = 64,
  rows = 50,
): Promise<MessageStoreRoomHead[]> {
  return invokeTauri("message_store_load_heads", { scope, rooms, rows });
}

export function readMessageStoreWindow(
  scope: MessageStoreScope,
  channelId: string,
  before: MessageStoreCursor | null,
  limit: number,
): Promise<MessageStoreWindowPage> {
  return invokeTauri("message_store_read_window", {
    scope,
    channelId,
    before,
    limit,
  });
}

export function readMessageStoreAround(
  scope: MessageStoreScope,
  channelId: string,
  anchor: { createdAt: number; eventId?: string },
  before: number,
  after: number,
): Promise<MessageStoreAroundPage> {
  return invokeTauri("message_store_read_around", {
    scope,
    channelId,
    anchor,
    before,
    after,
  });
}

export function readMessageStoreThread(
  scope: MessageStoreScope,
  rootId: string,
  cursor: MessageStoreCursor | null,
  limit: number,
): Promise<MessageStoreThreadPage> {
  return invokeTauri("message_store_read_thread", {
    scope,
    rootId,
    cursor,
    limit,
  });
}

export function getMessageStoreCoverage(
  scope: MessageStoreScope,
  channelIds: string[],
): Promise<MessageStoreRoomCoverage[]> {
  return invokeTauri("message_store_coverage", { scope, channelIds });
}

/** Registers a live subscription that reached EOSE; returns the segment id. */
export function beginMessageStoreLive(
  scope: MessageStoreScope,
  wsId: number,
  channelIds: string[],
  since: number,
): Promise<number> {
  return invokeTauri("message_store_begin_live", {
    scope,
    wsId,
    channelIds,
    since,
  });
}

export function clearMessageStoreScope(
  scope: MessageStoreScope,
): Promise<void> {
  return invokeTauri("message_store_clear_scope", { scope });
}
