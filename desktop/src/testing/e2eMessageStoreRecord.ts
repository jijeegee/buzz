/**
 * E2E mirror of `message_store/record.rs`: the mock fetch commands store
 * what they return and claim what the request and its bounds prove.
 */
import type {
  MessageStoreCursor,
  MessageStoreScope,
} from "@/shared/api/tauriMessageStore";
import type { RelayEvent } from "@/shared/api/types";
import { type MockClaim, mockMessageStoreScope } from "./e2eMessageStore.ts";

const KIND_WINDOW_BOUNDS = 39006;
const TIME_FLOOR_ID = "~";

const isHex64 = (value: unknown): value is string =>
  typeof value === "string" && /^[0-9a-fA-F]{64}$/.test(value);

const unixNow = () => Math.floor(Date.now() / 1000);

type RawCursor = { created_at: number; event_id: string };

function cursor(createdAt: number, eventId: string): MessageStoreCursor | null {
  return isHex64(eventId)
    ? { createdAt, eventId: eventId.toLowerCase() }
    : null;
}

function upper(requestCursor: RawCursor | null, fetchedAt: number) {
  if (!requestCursor) return { at: fetchedAt };
  const before = cursor(requestCursor.created_at, requestCursor.event_id);
  return before ? { before } : null;
}

/** `{has_more, next_cursor}` bounds content → claim (`bounds_claim`). */
export function boundsClaim(
  bounds: {
    has_more?: unknown;
    next_cursor?: { created_at?: unknown; id?: unknown } | null;
    direction?: unknown;
  },
  requestCursor: RawCursor | null,
  fetchedAt: number,
): MockClaim | null {
  if (bounds.direction !== undefined || typeof bounds.has_more !== "boolean") {
    return null;
  }
  let lo: MessageStoreCursor | null = null;
  if (bounds.has_more) {
    const next = bounds.next_cursor;
    if (
      !next ||
      typeof next.created_at !== "number" ||
      typeof next.id !== "string"
    ) {
      return null;
    }
    lo = cursor(next.created_at, next.id);
    if (!lo) return null;
  }
  const hi = upper(requestCursor, fetchedAt);
  return hi ? { lo, loIsStart: !bounds.has_more, hi } : null;
}

export function windowClaim(
  events: RelayEvent[],
  channelId: string,
  requestCursor: RawCursor | null,
  fetchedAt: number,
): MockClaim | null {
  const bounds = [...events]
    .reverse()
    .find(
      (event) =>
        event.kind === KIND_WINDOW_BOUNDS &&
        event.tags.some((tag) => tag[0] === "h" && tag[1] === channelId),
    );
  if (!bounds) return null;
  try {
    return boundsClaim(JSON.parse(bounds.content), requestCursor, fetchedAt);
  } catch {
    return null;
  }
}

export type MockRepairRequest = {
  since: number;
  limit: number;
  until?: number | null;
  beforeId?: string | null;
};

/** `repair_claim`: a short page reaches `since`; a full page its oldest event. */
export function repairClaim(
  events: RelayEvent[],
  request: MockRepairRequest,
  fetchedAt: number,
): MockClaim | null {
  let hi: MockClaim["hi"];
  if (request.until != null && request.beforeId != null) {
    const before = cursor(request.until, request.beforeId);
    if (!before) return null;
    hi = { before };
  } else if (request.until != null) {
    hi = { at: Math.min(request.until, fetchedAt) };
  } else {
    hi = { at: fetchedAt };
  }
  if (events.length < request.limit) {
    return {
      lo: { createdAt: request.since, eventId: TIME_FLOOR_ID },
      loIsStart: request.since === 0,
      hi,
    };
  }
  let oldest: MessageStoreCursor | null = null;
  for (const event of events) {
    const found = cursor(event.created_at, event.id);
    if (!found) return null;
    if (
      !oldest ||
      found.createdAt < oldest.createdAt ||
      (found.createdAt === oldest.createdAt && found.eventId > oldest.eventId)
    ) {
      oldest = found;
    }
  }
  return oldest ? { lo: oldest, loIsStart: false, hi } : null;
}

/** Store writes never fail a fetch. */
function record(
  scope: MessageStoreScope,
  write: (store: ReturnType<typeof mockMessageStoreScope>) => void,
) {
  try {
    write(mockMessageStoreScope(scope));
  } catch (error) {
    console.warn("e2e message store write failed", error);
  }
}

export function recordMockWindow(
  scope: MessageStoreScope,
  channelId: string,
  events: RelayEvent[],
  requestCursor: RawCursor | null,
  fetchedAt = unixNow(),
) {
  record(scope, (store) => {
    store.insertEvents(events, channelId);
    const claim = windowClaim(events, channelId, requestCursor, fetchedAt);
    if (claim) store.claimRoom(channelId, "top", claim);
  });
}

export function recordMockRepair(
  scope: MessageStoreScope,
  channelId: string,
  events: RelayEvent[],
  request: MockRepairRequest,
  fetchedAt = unixNow(),
) {
  record(scope, (store) => {
    store.insertEvents(events, channelId);
    const claim = repairClaim(events, request, fetchedAt);
    if (claim) store.claimRoom(channelId, "all", claim);
  });
}

export function recordMockThreadPages(
  scope: MessageStoreScope,
  pages: Array<{
    channel_id: string;
    root_id: string;
    events: RelayEvent[];
    has_more: boolean;
    next_cursor: RawCursor | null;
  }>,
  requests: Array<{
    channelId: string;
    rootId: string;
    cursor?: RawCursor | null;
  }>,
  fetchedAt = unixNow(),
) {
  record(scope, (store) => {
    for (const page of pages) {
      store.insertEvents(page.events, page.channel_id);
      const request = requests.find(
        (candidate) =>
          candidate.channelId === page.channel_id &&
          candidate.rootId === page.root_id,
      );
      if (!request || !isHex64(page.root_id)) continue;
      const claim = boundsClaim(
        {
          has_more: page.has_more,
          next_cursor: page.next_cursor
            ? {
                created_at: page.next_cursor.created_at,
                id: page.next_cursor.event_id,
              }
            : null,
        },
        request.cursor ?? null,
        fetchedAt,
      );
      if (claim) store.claimThread(page.root_id.toLowerCase(), claim);
    }
  });
}
