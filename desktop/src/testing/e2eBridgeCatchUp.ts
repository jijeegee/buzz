/**
 * E2E mirror of `commands/channels_catch_up.rs` and `message_store/sync.rs`:
 * one paged query per 128 rooms from the watermark, stored with an `all`
 * claim per room of a complete chunk; the watermark then moves to the start
 * time if the socket is still open.
 */
import type { ChannelCatchUpResult } from "@/shared/api/channelCatchUp";
import type { RelayEvent } from "@/shared/api/types";
import type { MockScopeStore } from "./e2eMessageStore.ts";

export const MOCK_CATCH_UP_LOOKBACK_SECS = 900 + 960 + 5;
const MAX_AGE_SECS = 24 * 60 * 60;
const CHUNK_ROOMS = 128;
const PAGE_LIMIT = 1000;
const MAX_PAGES = 3;
const TIME_FLOOR_ID = "~";

export type MockCatchUpFilter = {
  "#h": string[];
  kinds: number[];
  since: number;
  limit: number;
  until?: number;
  before_id?: string;
};

const roomOf = (event: RelayEvent) =>
  event.tags.find((tag) => tag[0] === "h")?.[1];

export async function mockChannelsCatchUp({
  store,
  tap,
  channelIds,
  dispatchChannelIds,
  syncKinds,
  query,
  now = () => Math.floor(Date.now() / 1000),
}: {
  store: MockScopeStore;
  tap: { markCaughtUp(): boolean } | undefined;
  channelIds: string[];
  dispatchChannelIds: string[];
  syncKinds: readonly number[];
  query: (filter: MockCatchUpFilter) => Promise<RelayEvent[]>;
  now?: () => number;
}): Promise<ChannelCatchUpResult> {
  const none = (status: ChannelCatchUpResult["status"]) => ({
    status,
    since: null,
    events: [],
    gapChannelIds: [],
  });
  const rooms = [...new Set([...channelIds, ...dispatchChannelIds])];
  if (!tap || rooms.length === 0) return none("unavailable");
  const startedAt = now();
  const watermark = store.watermark();
  if (watermark === null || startedAt - watermark > MAX_AGE_SECS) {
    if (tap.markCaughtUp()) store.advanceWatermark(startedAt);
    return none(watermark === null ? "fresh" : "stale");
  }
  const since = Math.max(0, watermark - MOCK_CATCH_UP_LOOKBACK_SECS);
  const dispatch = new Set(dispatchChannelIds);
  const events: RelayEvent[] = [];
  const gapChannelIds: string[] = [];
  for (let i = 0; i < rooms.length; i += CHUNK_ROOMS) {
    const chunk = rooms.slice(i, i + CHUNK_ROOMS);
    const fetchedAt = now();
    let cursor: { until: number; before_id: string } | null = null;
    let complete = false;
    const chunkEvents: RelayEvent[] = [];
    try {
      for (let page = 0; page < MAX_PAGES; page++) {
        const rows = await query({
          "#h": chunk,
          kinds: [...syncKinds],
          since,
          limit: PAGE_LIMIT,
          ...(cursor ?? {}),
        });
        chunkEvents.push(...rows);
        if (rows.length < PAGE_LIMIT) {
          complete = true;
          break;
        }
        const oldest = rows.reduce((a, b) =>
          b.created_at < a.created_at ||
          (b.created_at === a.created_at && b.id > a.id)
            ? b
            : a,
        );
        cursor = { until: oldest.created_at, before_id: oldest.id };
      }
    } catch (error) {
      console.warn("e2e catch-up query failed", error);
    }
    store.insertEvents(chunkEvents);
    if (complete) {
      for (const room of chunk) {
        store.claimRoom(room, "all", {
          lo: { createdAt: since, eventId: TIME_FLOOR_ID },
          loIsStart: since === 0,
          hi: { at: fetchedAt },
        });
      }
    } else {
      gapChannelIds.push(...chunk);
    }
    for (const event of chunkEvents) {
      const room = roomOf(event);
      if (room && dispatch.has(room)) events.push(event);
    }
  }
  if (tap.markCaughtUp()) store.advanceWatermark(startedAt);
  return { status: "caughtUp", since, events, gapChannelIds };
}
