import type { RelayEvent } from "@/shared/api/types";

type SubscriptionFilter = {
  "#h"?: readonly string[];
  "#p"?: readonly string[];
  kinds?: readonly number[];
};

/** Preserve raw REQ correlation alongside legacy mock delivery projections. */
export function createMockSubscription(filters: readonly SubscriptionFilter[]) {
  const channelIds = [
    ...new Set(filters.flatMap((filter) => filter["#h"] ?? [])),
  ];
  const kinds = [...new Set(filters.flatMap((filter) => filter.kinds ?? []))];
  return {
    filters,
    channelIds: channelIds.length ? channelIds : ["*"],
    kinds: kinds.length ? kinds : null,
    ownerPubkeys: [...new Set(filters.flatMap((filter) => filter["#p"] ?? []))],
  };
}

/** Test readiness must distinguish a channel consumer from unrelated global REQs. */
export function hasMockSubscription(
  subscriptions: Iterable<ReturnType<typeof createMockSubscription>>,
  channelId: string,
  kind?: number,
  exactChannel = false,
): boolean {
  for (const subscription of subscriptions) {
    if (exactChannel) {
      if (
        subscription.filters.some(
          (filter) =>
            filter["#h"]?.includes(channelId) &&
            (filter.kinds === undefined ||
              (filter.kinds.length > 0 &&
                (kind === undefined || filter.kinds.includes(kind)))),
        )
      )
        return true;
    } else if (
      (subscription.channelIds.includes(channelId) ||
        subscription.channelIds.includes("*")) &&
      (kind === undefined ||
        !subscription.kinds ||
        subscription.kinds.includes(kind))
    )
      return true;
  }
  return false;
}

/** What the store tap needs of one scope (`MockScopeStore`). */
type TapStore = {
  insertEvents(events: RelayEvent[]): number;
  noteFrame(at: number): void;
  beginLive(wsId: number, channelIds: string[], since: number): number;
  closeSegment(segmentId: number, closedAt: number): void;
  closeLive(wsId: number | null, closedAt: number): void;
  advanceWatermark(at: number): void;
};

const SEGMENT_FILTER_KEYS = new Set(["kinds", "#h", "since", "limit"]);
const KIND_THREAD_SUMMARY = 39005;

type PendingReq = {
  channelIds: string[];
  since: number | null;
  limit: number | null;
  sentAt: number;
  replayed: number;
};

/**
 * E2E mirror of `native_websocket_store_tap.rs`, at mock frame delivery:
 * room events reach the store before the app sees them; a subscription for
 * every synced kind of its rooms becomes a live segment at EOSE and closes on
 * CLOSE/CLOSED/replacement or when the socket closes.
 */
export function createMockStoreTap(
  store: TapStore,
  wsId: number,
  syncKinds: readonly number[],
  now: () => number = () => Math.floor(Date.now() / 1000),
) {
  const synced = new Set<number>([...syncKinds, KIND_THREAD_SUMMARY]);
  const pending = new Map<string, PendingReq>();
  const segments = new Map<string, number>();
  let lastFrameAt: number | null = null;
  let finished = false;
  // Watermark (`message_store::sync`): moved only once a catch-up ran.
  let caughtUp = false;
  const roundTrips = new Map<string, number>();
  const vouch = (at: number) => {
    if (caughtUp) store.advanceWatermark(at);
  };

  const end = (subId: string) => {
    pending.delete(subId);
    roundTrips.delete(subId);
    const segment = segments.get(subId);
    if (segment === undefined) return false;
    segments.delete(subId);
    store.closeSegment(segment, lastFrameAt ?? now());
    return true;
  };

  const segmentRequest = (filters: unknown[]): PendingReq | null => {
    const [filter] = filters;
    if (filters.length !== 1 || !filter || typeof filter !== "object") {
      return null;
    }
    const f = filter as Record<string, unknown>;
    if (Object.keys(f).some((key) => !SEGMENT_FILTER_KEYS.has(key))) {
      return null;
    }
    const kinds = Array.isArray(f.kinds) ? f.kinds : [];
    const rooms = f["#h"];
    if (
      !syncKinds.every((kind) => kinds.includes(kind)) ||
      !Array.isArray(rooms) ||
      rooms.length === 0 ||
      !rooms.every((room) => typeof room === "string")
    ) {
      return null;
    }
    return {
      channelIds: rooms as string[],
      since: typeof f.since === "number" ? f.since : null,
      limit: typeof f.limit === "number" ? f.limit : null,
      sentAt: now(),
      replayed: 0,
    };
  };

  const parse = (text: string) => {
    try {
      const frame = JSON.parse(text);
      return Array.isArray(frame) && typeof frame[1] === "string"
        ? (frame as [string, string, ...unknown[]])
        : null;
    } catch {
      return null;
    }
  };

  return {
    /** Every frame the relay delivers (`null` for non-text frames). */
    incoming(text: string | null) {
      if (finished) return;
      lastFrameAt = now();
      const frame = text === null ? null : parse(text);
      let touched = false;
      if (frame) {
        const [type, subId, event] = frame;
        if (type === "EVENT") {
          const request = pending.get(subId);
          if (request) request.replayed += 1;
          const e = event as RelayEvent | undefined;
          if (
            e &&
            synced.has(e.kind) &&
            Array.isArray(e.tags) &&
            e.tags.some((tag) => tag[0] === "h" && Boolean(tag[1]))
          ) {
            store.insertEvents([e]);
            if (segments.has(subId))
              vouch(Math.min(e.created_at, lastFrameAt ?? now()));
            touched = true;
          }
        } else if (type === "EOSE") {
          const sentAt = roundTrips.get(subId);
          roundTrips.delete(subId);
          if (sentAt !== undefined) vouch(sentAt);
          const request = pending.get(subId);
          if (request) {
            pending.delete(subId);
            const floor = request.since ?? Number.MIN_SAFE_INTEGER;
            const complete =
              request.since !== null &&
              request.limit !== null &&
              request.replayed < request.limit;
            const since = complete ? floor : Math.max(request.sentAt, floor);
            const segment = store.beginLive(wsId, request.channelIds, since);
            const replaced = segments.get(subId);
            segments.set(subId, segment);
            if (replaced !== undefined) store.closeSegment(replaced, since);
            touched = true;
          }
        } else if (type === "CLOSED") {
          touched = end(subId);
        }
      }
      // Like the native tap, the frame time is written with the work it
      // came with, so a crash closes open segments no later than it.
      if (touched) store.noteFrame(lastFrameAt);
    },
    /** Every text frame the app sends. */
    outgoing(text: string) {
      if (finished) return;
      const frame = parse(text);
      if (!frame || (frame[0] !== "REQ" && frame[0] !== "CLOSE")) return;
      const [type, subId, ...filters] = frame;
      end(subId);
      if (type === "REQ") {
        roundTrips.set(subId, now());
        const request = segmentRequest(filters);
        if (request) pending.set(subId, request);
      }
    },
    /** A catch-up committed for this socket; false once it closed. */
    markCaughtUp() {
      if (finished) return false;
      caughtUp = true;
      return true;
    },
    /** The socket closed: its segments close at its last frame. */
    finish() {
      if (finished) return;
      finished = true;
      pending.clear();
      roundTrips.clear();
      segments.clear();
      if (lastFrameAt !== null) store.closeLive(wsId, lastFrameAt);
    },
  };
}
