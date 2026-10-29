/**
 * Coverage of the device message store: which spans of time of a room are
 * known to be stored completely, and catching up the rest from the relay.
 * Rules: docs/message-sync.md#coverage.
 */
import { getChannelReconnectRepairEvents } from "@/shared/api/channelReconnectRepair";
import {
  MAX_EXPLICIT_CHANNEL_VALUES,
  type RelaySubscription,
  type RelaySubscriptionFilter,
} from "@/shared/api/relayClientShared";
import {
  isRateLimited,
  waitForRateLimit,
} from "@/shared/api/relayRateLimitGate";
import { invokeTauri } from "@/shared/api/tauri";
import type { RelayEvent } from "@/shared/api/types";
import { CHANNEL_EVENT_KINDS } from "@/shared/constants/kinds";

/** Same margin as reconnect repair: relay future tolerance plus storage fence. */
export const STORE_RESUME_LOOKBACK_SECS = 900 + 960 + 5;
/** A live REQ records its coverage at most this often. */
export const STORE_MARK_INTERVAL_SECS = 60;
export const STORE_CATCH_UP_PAGE_LIMIT = 500;
/** Beyond this the gap is left for the room to fill when it is opened. */
export const STORE_CATCH_UP_MAX_PAGES = 4;
export const STORE_CATCH_UP_MAX_AGE_SECS = 7 * 24 * 60 * 60;

type LiveSubscription = Extract<RelaySubscription, { mode: "live" }>;

type CoverageDeps = {
  nowSecs: () => number;
  /** The store scope coverage is recorded for now (`null`: none). */
  scope: () => Promise<string | null>;
  /** Refused when `scope` is no longer current. */
  extend: (
    roomIds: string[],
    from: number,
    to: number,
    scope: string,
  ) => Promise<void>;
  resumePoints: (
    roomIds: string[],
    before: number,
  ) => Promise<Record<string, number>>;
  requestPage: (request: {
    channelIds: string[];
    since: number;
    limit: number;
    until?: number;
    beforeId?: string;
  }) => Promise<RelayEvent[]>;
};

const defaultDeps: CoverageDeps = {
  nowSecs: () => Math.floor(Date.now() / 1000),
  scope: () => invokeTauri<string | null>("message_store_scope"),
  extend: (roomIds, from, to, scope) =>
    invokeTauri<void>("message_store_extend_coverage", {
      roomIds,
      from,
      to,
      scope,
    }),
  resumePoints: async (roomIds, before) => {
    const points = await invokeTauri<Record<string, number> | null>(
      "message_store_resume_points",
      { roomIds, before },
    );
    return points && typeof points === "object" ? points : {};
  },
  requestPage: getChannelReconnectRepairEvents,
};

let deps = defaultDeps;

/** Covered live REQs that reached EOSE; retired ones drop their coverage. */
const liveSubscriptions = new Set<LiveSubscription>();

/** Replace the store bridge (tests). Returns the restore function. */
export function setStoreCoverageDeps(next: Partial<CoverageDeps>) {
  const previous = deps;
  deps = { ...deps, ...next };
  return () => {
    deps = previous;
  };
}

function record(
  roomIds: string[],
  from: number,
  to: number,
  scope: Promise<string | null>,
) {
  // No scope (signed out at the time): nothing is recorded.
  void scope
    .then((current) =>
      current === null ? undefined : deps.extend(roomIds, from, to, current),
    )
    .catch(() => {});
}

/** The scope of a span with no EOSE to pin it to: whatever is current. */
function currentScope() {
  return deps.scope().catch(() => null);
}

/** Room ids of a live REQ carrying every channel event of explicit rooms. */
export function storeCoveredRooms(filter: RelaySubscriptionFilter) {
  const roomIds = filter["#h"];
  if (
    !Array.isArray(roomIds) ||
    roomIds.length === 0 ||
    roomIds.length > MAX_EXPLICIT_CHANNEL_VALUES ||
    !CHANNEL_EVENT_KINDS.every((kind) => filter.kinds.includes(kind))
  ) {
    return null;
  }
  return roomIds;
}

/**
 * A covered live REQ reached EOSE on connection `generation`: from now on it
 * receives every event of its rooms. Unless a reconnect repair is filling
 * this connection's gap, catch the stored rooms up from where they stopped.
 */
export function noteStoreLiveEose(
  subscription: LiveSubscription,
  generation: number,
) {
  const roomIds = storeCoveredRooms(subscription.filter);
  if (!roomIds) return;
  const liveAt = deps.nowSecs();
  const scope = currentScope();
  const coverage = { generation, liveAt, markedAt: liveAt, scope };
  subscription.storeCoverage = coverage;
  liveSubscriptions.add(subscription);
  record(roomIds, liveAt, liveAt, scope);
  if (subscription.reconnectReplay?.generation !== generation) {
    // Stops once this REQ's coverage is retired or replaced.
    const live = () => subscription.storeCoverage === coverage;
    void catchUpFromStore(roomIds, liveAt, scope, live).catch(() => {});
  }
}

/**
 * When the live REQ carrying `roomId` reached EOSE, if one is receiving now:
 * from then on the store has every event of the room as it arrives.
 */
export function storeLiveAt(roomId: string): number | null {
  let liveAt: number | null = null;
  for (const { coverage } of storeLiveCoverage(roomId)) {
    if (liveAt === null || coverage.liveAt < liveAt) liveAt = coverage.liveAt;
  }
  return liveAt;
}

export type StoreLiveCoverage = {
  subscription: LiveSubscription;
  coverage: NonNullable<LiveSubscription["storeCoverage"]>;
};

/**
 * The live REQs carrying `roomId` that reached EOSE and have received
 * everything since. Each coverage object is made at one EOSE and cleared
 * when its REQ stops receiving (retired, CLOSED, socket lost), never reused:
 * while `subscription.storeCoverage === coverage`, that REQ has been live
 * without a gap since.
 */
export function storeLiveCoverage(roomId: string): StoreLiveCoverage[] {
  const live: StoreLiveCoverage[] = [];
  for (const subscription of liveSubscriptions) {
    const coverage = subscription.storeCoverage;
    if (!coverage) {
      liveSubscriptions.delete(subscription);
      continue;
    }
    if (storeCoveredRooms(subscription.filter)?.includes(roomId)) {
      live.push({ subscription, coverage });
    }
  }
  return live;
}

/** An event arrived on `generation`: the REQ was still live, so extend. */
export function noteStoreLiveEvent(
  subscription: LiveSubscription,
  generation: number,
) {
  const coverage = subscription.storeCoverage;
  if (!coverage || coverage.generation !== generation) return;
  const now = deps.nowSecs();
  if (now - coverage.markedAt < STORE_MARK_INTERVAL_SECS) return;
  const roomIds = storeCoveredRooms(subscription.filter);
  if (!roomIds) return;
  coverage.markedAt = now;
  record(roomIds, coverage.liveAt, now, coverage.scope);
}

/** A reconnect repair walked every event of the REQ's rooms since `since`. */
export function noteStoreRepairCompleted(
  subscription: LiveSubscription,
  since: number,
) {
  const roomIds = storeCoveredRooms(subscription.filter);
  const scope = subscription.storeCoverage?.scope ?? currentScope();
  if (roomIds) record(roomIds, since, deps.nowSecs(), scope);
}

/**
 * Fetch what the store is missing for `roomIds` between their last coverage
 * and `liveAt`, into the store only (nothing is shown). Rooms never covered
 * are skipped; a gap too old or too large is left to opening the room.
 */
export async function catchUpFromStore(
  roomIds: string[],
  liveAt: number,
  scope: Promise<string | null> = currentScope(),
  live: () => boolean = () => true,
) {
  // Spans opened at liveAt (this REQ's own) may be written already.
  const points = await deps.resumePoints(roomIds, liveAt);
  const covered = roomIds.filter((roomId) => points[roomId] !== undefined);
  if (covered.length === 0) return false;
  const oldest = Math.min(...covered.map((roomId) => points[roomId]));
  const since = Math.max(0, oldest - STORE_RESUME_LOOKBACK_SECS);
  if (since < liveAt - STORE_CATCH_UP_MAX_AGE_SECS) return false;
  // The first page starts at the relay's newest, not at this PC's clock: rows
  // the relay stamped after liveAt but sent before EOSE are fetched too.
  let until: number | undefined;
  let beforeId: string | undefined;
  for (let page = 0; page < STORE_CATCH_UP_MAX_PAGES; page++) {
    if (isRateLimited()) await waitForRateLimit();
    if (!live()) return false;
    const events = await deps.requestPage({
      channelIds: covered,
      since,
      limit: STORE_CATCH_UP_PAGE_LIMIT,
      until,
      beforeId,
    });
    const terminal = events.at(-1);
    if (events.length < STORE_CATCH_UP_PAGE_LIMIT || !terminal) {
      if (!live()) return false;
      record(covered, since, liveAt, scope);
      return true;
    }
    if (terminal.created_at === until && terminal.id === beforeId) return false;
    until = terminal.created_at;
    beforeId = terminal.id;
  }
  return false;
}
