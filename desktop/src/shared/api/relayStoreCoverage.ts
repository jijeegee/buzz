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
import { namesUserStream } from "@/shared/api/userStreamRules";
import { windowChecksActive } from "@/features/messages/lib/windowCheckMode";

/** Same margin as reconnect repair: relay future tolerance plus storage fence. */
export const STORE_RESUME_LOOKBACK_SECS = 900 + 960 + 5;
/** A live REQ records its coverage at most this often. */
export const STORE_MARK_INTERVAL_SECS = 60;
export const STORE_CATCH_UP_PAGE_LIMIT = 500;
/** Beyond this the gap is left for the room to fill when it is opened. */
export const STORE_CATCH_UP_MAX_PAGES = 4;
export const STORE_CATCH_UP_MAX_AGE_SECS = 7 * 24 * 60 * 60;
/** How long the idle preload waits for its rooms' live REQs to settle. */
export const STORE_LIVE_WAIT_MS = 3_000;
const STORE_LIVE_POLL_MS = 100;

type LiveSubscription = Extract<RelaySubscription, { mode: "live" }>;

/** Rule-W rows riding a span: each room's snapshot seq, under `epoch`. */
export type StreamSeqWrite = { epoch: string; seqs: Record<string, number> };

/**
 * What the user stream adds to a chunk catch-up that starts after its
 * shadow EOSE on the same connection (docs/message-sync.md#counters):
 * rule-W rows for the span, and the walk's rows when it completes.
 */
export type StreamCatchUpVouch = {
  seq: StreamSeqWrite | null;
  /** The walk ended; `events` are its rows when it completed. */
  ended: (completed: boolean, events: RelayEvent[]) => void;
};
let streamVouch:
  | ((roomIds: string[], liveAt: number) => StreamCatchUpVouch | null)
  | null = null;

/** The shadow stream's hook into chunk catch-ups (`null`: none). */
export function setStreamCatchUpVouch(next: typeof streamVouch) {
  streamVouch = next;
}

type CoverageDeps = {
  nowSecs: () => number;
  nowMs: () => number;
  /** Resolves after `ms`; `cancel` resolves it at once. */
  sleep: (ms: number) => { done: Promise<void>; cancel: () => void };
  /**
   * The store capture coverage is recorded with (`null`: none): the one the
   * backend took when subscription `subId` reached EOSE, or one taken now.
   */
  scope: (subId?: string) => Promise<string | null>;
  /** Refused when `scope` is no longer current; rule-W rows with `seq`. */
  extend: (
    roomIds: string[],
    from: number,
    to: number,
    scope: string,
    seq?: StreamSeqWrite,
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
  /** Drops the head and page spans of rooms a catch-up left a gap in. */
  forgetWindows: (roomIds: string[], scope: string | null) => Promise<void>;
  /** Whether windows may open by a check (else the store is left as today). */
  windowChecksActive: () => boolean;
};

const defaultDeps: CoverageDeps = {
  nowSecs: () => Math.floor(Date.now() / 1000),
  nowMs: () => Date.now(),
  sleep: (ms) => {
    let cancel = () => {};
    const done = new Promise<void>((resolve) => {
      const timer = setTimeout(resolve, ms);
      cancel = () => {
        clearTimeout(timer);
        resolve();
      };
    });
    return { done, cancel };
  },
  scope: (subId) =>
    invokeTauri<string | null>("message_store_scope", { subId: subId ?? null }),
  extend: (roomIds, from, to, scope, seq) =>
    seq
      ? invokeTauri<void>("message_store_extend_stream_coverage", {
          roomIds,
          from,
          to,
          scope,
          epoch: seq.epoch,
          seqs: seq.seqs,
        })
      : invokeTauri<void>("message_store_extend_coverage", {
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
  forgetWindows: (roomIds, scope) =>
    invokeTauri<void>("message_store_forget_windows", { roomIds, scope }),
  windowChecksActive,
};

let deps = defaultDeps;

/** Covered live REQs that reached EOSE; retired ones drop their coverage. */
const liveSubscriptions = new Set<LiveSubscription>();
/**
 * Per EOSE coverage, the catch-up (or reconnect repair) filling the gap
 * before it; settles when the store holds what the REQ missed.
 */
const settling = new WeakMap<object, Promise<unknown>>();
/** A reconnect repair in flight per REQ: resolved when it completes. */
const repairs = new WeakMap<LiveSubscription, () => void>();
/**
 * The user stream's coverage while it carries rooms in live/gated
 * (docs/message-sync.md#user-stream): a stand-in REQ whose `storeCoverage`
 * is cleared when the stream stops, so tokens taken under it stop holding.
 */
let stream: { subscription: LiveSubscription; rooms: Set<string> } | null =
  null;

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
  seq?: StreamSeqWrite,
) {
  if (roomIds.length === 0) return;
  // No scope (signed out at the time): nothing is recorded.
  void scope
    .then((current) =>
      current === null
        ? undefined
        : seq
          ? deps.extend(roomIds, from, to, current, seq)
          : deps.extend(roomIds, from, to, current),
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
    namesUserStream(filter) ||
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
  subId?: string,
) {
  const roomIds = storeCoveredRooms(subscription.filter);
  if (!roomIds) return;
  const liveAt = deps.nowSecs();
  // Taken by the backend as the socket delivered the EOSE.
  const scope = deps.scope(subId).catch(() => null);
  const coverage = { generation, liveAt, markedAt: liveAt, scope };
  subscription.storeCoverage = coverage;
  liveSubscriptions.add(subscription);
  record(roomIds, liveAt, liveAt, scope);
  if (subscription.reconnectReplay?.generation !== generation) {
    // Stops once this REQ's coverage is retired or replaced.
    const live = () => subscription.storeCoverage === coverage;
    // Rooms the stream carries are caught up by the stream (`chunkCatchUp`).
    const own = roomIds.filter((roomId) => !streamCarries(roomId));
    settling.set(
      coverage,
      catchUpFromStore(own, liveAt, scope, live).catch(() => {}),
    );
  } else if (!subscription.reconnectReplay.repairDone) {
    // A waiter on an earlier EOSE of this REQ stops waiting.
    noteStoreRepairEnded(subscription);
    settling.set(
      coverage,
      new Promise<void>((resolve) => repairs.set(subscription, resolve)),
    );
  }
}

/**
 * Waits, at most `capMs`, until a live REQ carrying each of `roomIds` has
 * reached EOSE and caught the store up on what it missed (the idle
 * preload's store check; docs/message-sync.md#store-first-preload).
 */
export async function waitStoreLive(
  roomIds: readonly string[],
  capMs = STORE_LIVE_WAIT_MS,
  signal?: AbortSignal,
): Promise<void> {
  const deadline = deps.nowMs() + capMs;
  // Every sleep ends early when `signal` aborts.
  const nap = async (ms: number, until?: Promise<unknown>) => {
    if (signal?.aborted) return;
    const timer = deps.sleep(ms);
    const stop = () => timer.cancel();
    signal?.addEventListener("abort", stop, { once: true });
    await (until ? Promise.race([until, timer.done]) : timer.done);
    signal?.removeEventListener("abort", stop);
    timer.cancel();
  };
  while (roomIds.some((roomId) => storeLiveCoverage(roomId).length === 0)) {
    const left = deadline - deps.nowMs();
    if (left <= 0 || signal?.aborted) return;
    await nap(Math.min(STORE_LIVE_POLL_MS, left));
  }
  const pending = roomIds.flatMap((roomId) =>
    storeLiveCoverage(roomId).map(({ coverage }) => settling.get(coverage)),
  );
  const left = deadline - deps.nowMs();
  if (left <= 0) return;
  await nap(left, Promise.all(pending));
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
  const streamCoverage = stream?.subscription.storeCoverage;
  if (stream && streamCoverage && stream.rooms.has(roomId)) {
    live.push({ subscription: stream.subscription, coverage: streamCoverage });
  }
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
  noteStoreRepairEnded(subscription);
}

/**
 * A reconnect repair of `subscription` ended, completed or not: a preload
 * waiting on it stops waiting (an unrepaired room is simply not certain).
 */
export function noteStoreRepairEnded(subscription: LiveSubscription) {
  repairs.get(subscription)?.();
  repairs.delete(subscription);
}

/**
 * A catch-up skipped `roomIds` (too old) or cut them short (page cap): the
 * store has a gap it cannot see, so their stored windows are fetched, not
 * checked, when next opened (docs/message-sync.md#window-check). Only
 * while checks may run: with every relay off the store stays as today. The
 * store's `scope` (account and server) is the one the rooms were read in.
 */
export function forgetStoreWindows(
  roomIds: readonly string[],
  scope: Promise<string | null>,
) {
  if (roomIds.length === 0 || !deps.windowChecksActive()) return;
  const rooms = [...roomIds];
  void scope
    .then((token) => deps.forgetWindows(rooms, token))
    .catch(() => {
      // No store bridge (tests without Tauri): nothing to forget.
    });
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
  if (since < liveAt - STORE_CATCH_UP_MAX_AGE_SECS) {
    forgetStoreWindows(covered, scope);
    return false;
  }
  // The first page starts at the relay's newest, not at this PC's clock: rows
  // the relay stamped after liveAt but sent before EOSE are fetched too.
  let until: number | undefined;
  let beforeId: string | undefined;
  // Taken before the first page (rule W: it started after the stream EOSE).
  const vouch = streamVouch?.(covered, liveAt) ?? null;
  const walked: RelayEvent[] = [];
  const end = (completed: boolean) => {
    vouch?.ended(completed, walked);
    return completed;
  };
  for (let page = 0; page < STORE_CATCH_UP_MAX_PAGES; page++) {
    if (isRateLimited()) await waitForRateLimit();
    if (!live()) return end(false);
    let events: RelayEvent[];
    try {
      events = await deps.requestPage({
        channelIds: covered,
        since,
        limit: STORE_CATCH_UP_PAGE_LIMIT,
        until,
        beforeId,
      });
    } catch (error) {
      end(false);
      throw error;
    }
    if (vouch) walked.push(...events);
    const terminal = events.at(-1);
    if (events.length < STORE_CATCH_UP_PAGE_LIMIT || !terminal) {
      if (!live()) return end(false);
      record(covered, since, liveAt, scope, vouch?.seq ?? undefined);
      return end(true);
    }
    if (terminal.created_at === until && terminal.id === beforeId) {
      forgetStoreWindows(covered, scope);
      return end(false);
    }
    until = terminal.created_at;
    beforeId = terminal.id;
  }
  forgetStoreWindows(covered, scope);
  return end(false);
}

/**
 * The stream's accepted live/gated EOSE at `liveAt`: it carries `rooms`
 * (kept by reference: 44100/44101 change it), in memory only. A room's
 * stream span `[L, …]` is recorded once its catch-up batch completed
 * (`spanned`), never for a room whose catch-up was cut short
 * (docs/message-sync.md#catch-up-at-the-stream-eose).
 * `settled` is its catch-up, which the preload wait waits for.
 */
export function noteStreamLiveEose({
  generation,
  liveAt,
  rooms,
  scope,
}: {
  generation: number;
  liveAt: number;
  rooms: Set<string>;
  scope: Promise<string | null>;
}) {
  endStreamCoverage();
  const coverage = { generation, liveAt, markedAt: liveAt, scope };
  const subscription: LiveSubscription = {
    mode: "live",
    filter: { kinds: [...CHANNEL_EVENT_KINDS], limit: 0 },
    onEvent: () => {},
    storeCoverage: coverage,
  };
  stream = { subscription, rooms };
  /** Carried room -> when the stream began carrying it. */
  const starts = new Map<string, number>();
  /** Handed rooms whose catch-up at the EOSE is not done: no span yet. */
  const pending = new Set<string>();
  /** Carried rooms whose stream span is in the store. */
  const spanned = new Set<string>();
  /** `[start, to]` for each of `roomIds`, grouped by start. */
  const recordFrom = (roomIds: Iterable<string>, to: number) => {
    const byStart = new Map<number, string[]>();
    for (const roomId of roomIds) {
      const start = starts.get(roomId) ?? liveAt;
      byStart.set(start, [...(byStart.get(start) ?? []), roomId]);
    }
    for (const [start, ids] of byStart) {
      record(ids, start, Math.max(start, to), scope);
    }
  };
  return {
    /** Rooms handed to the stream once their resume points are read. */
    hand: (roomIds: string[]) => {
      if (subscription.storeCoverage !== coverage) return;
      for (const roomId of roomIds) {
        rooms.add(roomId);
        pending.add(roomId);
        if (!starts.has(roomId)) starts.set(roomId, liveAt);
      }
    },
    /** Rooms whose catch-up is done (walked, or skipped in gated). */
    span: (roomIds: readonly string[]) => {
      for (const roomId of roomIds) pending.delete(roomId);
      // After a CLOSED no span is written (the fallback walks instead).
      if (subscription.storeCoverage !== coverage) return;
      const done = roomIds.filter(
        (roomId) => rooms.has(roomId) && !spanned.has(roomId),
      );
      for (const roomId of done) spanned.add(roomId);
      recordFrom(done, coverage.markedAt);
    },
    /** The catch-up the preload waits for. */
    settle: (settled: Promise<unknown>) => settling.set(coverage, settled),
    /** A stream frame: extends the spans at most once per minute. */
    extend: (to: number) => {
      if (subscription.storeCoverage !== coverage) return;
      if (to - coverage.markedAt < STORE_MARK_INTERVAL_SECS) return;
      coverage.markedAt = to;
      recordFrom(
        [...spanned].filter((roomId) => rooms.has(roomId)),
        to,
      );
    },
    /**
     * A room the stream starts carrying (44100): its span from `at`, once
     * its catch-up at the EOSE is done when it still has one.
     */
    carry: (roomId: string, at: number) => {
      if (subscription.storeCoverage !== coverage) return;
      rooms.add(roomId);
      starts.set(roomId, at);
      spanned.delete(roomId);
      if (pending.has(roomId)) return;
      spanned.add(roomId);
      record([roomId], at, at, scope);
    },
  };
}

/** Whether the stream carries `roomId` now (its catch-up is the stream's). */
export function streamCarries(roomId: string) {
  return (
    stream?.subscription.storeCoverage !== undefined && stream.rooms.has(roomId)
  );
}

/** The stream stopped (CLOSED, socket end): its spans stop growing. */
export function endStreamCoverage() {
  if (stream) stream.subscription.storeCoverage = undefined;
  stream = null;
}
