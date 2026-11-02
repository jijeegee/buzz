import type { RelayEvent } from "@/shared/api/types";
import {
  CHANNEL_EVENT_KINDS,
  KIND_CHANNEL_THREAD_SUMMARY,
  KIND_THREAD_NAME,
} from "@/shared/constants/kinds";
import {
  MAX_EXPLICIT_CHANNEL_VALUES,
  type LiveSubscriptionReadiness,
  type RelaySubscriptionFilter,
} from "@/shared/api/relayClientShared";

/**
 * Room metadata the bundle carries for every covered room (thread names).
 * Delivered only to metadata listeners (`listenRoomMetadata`): never to room
 * listeners, unread, the sidebar or the chat list.
 */
export const ROOM_METADATA_KINDS: readonly number[] = [KIND_THREAD_NAME];

/**
 * Kinds every room bundle carries: the room's own events, the relay's live
 * thread-summary recounts (39005) and room metadata. Summaries are delivered
 * only to room listeners, never to the all-rooms listeners (unread, sidebar,
 * chat list).
 */
export const ROOM_LIVE_KINDS: readonly number[] = [
  ...CHANNEL_EVENT_KINDS,
  KIND_CHANNEL_THREAD_SUMMARY,
  ...ROOM_METADATA_KINDS,
];

/** The longest `whenRoomCovered` waits. */
export const ROOM_COVERAGE_WAIT_MS = 1_500;

/**
 * How long a room that leaves the member and quiet sets stays in its chunk,
 * so a hide then unhide (or a list refetch that briefly drops a room) does
 * not re-send the chunk REQ.
 */
export const ROOM_SHRINK_GRACE_MS = 1_000;

/** Events remembered for duplicate suppression across overlapping deliveries. */
const SEEN_EVENT_LIMIT = 20_000;
/** Event id → room, so h-less reactions and deletions find their room. */
const EVENT_ROOM_LIMIT = 50_000;
const RETRY_BASE_MS = 1_000;
const RETRY_MAX_MS = 30_000;

export type RoomLiveListener = (event: RelayEvent) => void;

export type OpenRoomBundleSubscription = (
  filter: RelaySubscriptionFilter,
  onEvent: (event: RelayEvent) => void,
  onReady: (readiness: LiveSubscriptionReadiness) => void,
  /** Aborted when the chunk is retired before its REQ goes out. */
  signal: AbortSignal,
) => Promise<() => Promise<void>>;

type ChunkStatus = "opening" | "live" | "failed";

type Chunk = {
  key: string;
  rooms: ReadonlySet<string>;
  status: ChunkStatus;
  failures: number;
  dispose: (() => Promise<void>) | null;
  retryTimer: number | null;
  controller: AbortController;
};

export type RoomBundleTimers = {
  setTimeout: (fn: () => void, ms: number) => number;
  clearTimeout: (id: number) => void;
};

type RoomWaiter = {
  roomId: string;
  resolve: () => void;
};

type CoverWaiter = {
  roomIds: readonly string[];
  resolve: (covered: boolean) => void;
  timer: number;
};

/** The relay accepts at most this many explicit channels per REQ. */
export function chunkRoomIds(roomIds: Iterable<string>): string[][] {
  const sorted = [...new Set(roomIds)].sort();
  const chunks: string[][] = [];
  for (
    let start = 0;
    start < sorted.length;
    start += MAX_EXPLICIT_CHANNEL_VALUES
  ) {
    chunks.push(sorted.slice(start, start + MAX_EXPLICIT_CHANNEL_VALUES));
  }
  return chunks;
}

function sameSet(a: ReadonlySet<string>, b: ReadonlySet<string>) {
  return a.size === b.size && [...a].every((value) => b.has(value));
}

function tagValue(event: RelayEvent, name: string) {
  return event.tags.find((tag) => tag[0] === name)?.[1] ?? null;
}

/** Referenced ids, last first: NIP-25 names the reacted event last. */
function referencedIds(event: RelayEvent) {
  const ids: string[] = [];
  for (const tag of event.tags) {
    if (tag[0] === "e" && typeof tag[1] === "string") ids.push(tag[1]);
  }
  return ids.reverse();
}

function withRoomTag(event: RelayEvent, roomId: string): RelayEvent {
  return tagValue(event, "h")
    ? event
    : { ...event, tags: [...event.tags, ["h", roomId]] };
}

function remember<T>(
  map: Map<string, T>,
  key: string,
  value: T,
  limit: number,
) {
  map.delete(key);
  map.set(key, value);
  if (map.size > limit) {
    const oldest = map.keys().next().value;
    if (oldest !== undefined) map.delete(oldest);
  }
}

/**
 * One live subscription for every room instead of one per room.
 *
 * Member rooms (the sidebar's rooms, DMs and groups; their threads ride the
 * same `h` tag) are packed into REQs of up to 128 rooms. A room opened without
 * membership (a browsed open channel) gets its own chunk, so losing access to
 * it still closes a subscription the client can see, as before.
 *
 * Quiet rooms (hidden DMs) ride the same chunks. Their events reach room
 * listeners and quiet listeners only, never the all-rooms listeners: a hidden
 * DM counts as unread only after it is shown again (a member room).
 *
 * Every event is delivered once: overlapping chunks (while one replaces
 * another) and reconnect repair can carry the same event, so duplicates are
 * dropped here before any listener runs. A replaced chunk is closed only
 * after the chunk taking over its rooms is live, so a room is never without
 * coverage during the swap.
 *
 * Reactions and their removals carry no `h` tag. A single-room chunk names the
 * room directly; otherwise the room comes from the event they reference, as
 * seen here earlier or as known to `resolveRoomOfEvent` (the loaded
 * timelines). An event whose room cannot be told is dropped: it references
 * nothing on screen, and a page that later loads its target carries it.
 */
export class RoomLiveBundle {
  private memberRooms = new Set<string>();
  private quietRooms = new Set<string>();
  /** Rooms that just left member ∪ quiet → their grace timer. */
  private graceRetained = new Map<string, number>();
  private extraRooms = new Map<string, number>();
  /** Rooms the user stream carries in live/gated: no chunk for them. */
  private streamRooms: ReadonlySet<string> | null = null;
  /**
   * `false` while a stopped stream waits for its socket to close (overflow,
   * `lagged`): its rooms stay off chunks but are neither live nor settled.
   */
  private streamLive = true;
  private chunks = new Map<string, Chunk>();
  private roomListeners = new Map<string, Set<RoomLiveListener>>();
  private allRoomListeners = new Set<RoomLiveListener>();
  private metadataListeners = new Set<RoomLiveListener>();
  private quietListeners = new Set<RoomLiveListener>();
  private resetListeners = new Set<() => void>();
  private chunkObservers = new Set<
    (event: RelayEvent, roomId: string) => void
  >();
  private waiters = new Set<RoomWaiter>();
  private coverWaiters = new Set<CoverWaiter>();
  /** Set by the first non-empty member list since the last reset. */
  private membershipKnown = false;
  private seen = new Map<string, true>();
  private eventRooms = new Map<string, string>();
  private generation = 0;
  private syncQueued = false;
  private resolveRoomOfEvent: (eventId: string) => string | null = () => null;
  private readonly openSubscription: OpenRoomBundleSubscription;
  private readonly timers: RoomBundleTimers;

  constructor(
    openSubscription: OpenRoomBundleSubscription,
    timers: RoomBundleTimers,
  ) {
    this.openSubscription = openSubscription;
    this.timers = timers;
  }

  /** Rooms the account belongs to; their events also reach all-rooms listeners. */
  setMemberRooms(roomIds: Iterable<string>) {
    const next = new Set(roomIds);
    if (next.size > 0) this.membershipKnown = true;
    if (!sameSet(next, this.memberRooms)) {
      const before = this.carriedRooms();
      this.memberRooms = next;
      this.afterCarriedChange(before);
    }
    this.settleCoverWaiters();
  }

  /**
   * Rooms carried without being listed (hidden DMs). Their events reach room
   * and quiet listeners only. A room both member and quiet is a member.
   */
  setQuietRooms(roomIds: Iterable<string>) {
    const next = new Set(roomIds);
    if (!sameSet(next, this.quietRooms)) {
      const before = this.carriedRooms();
      this.quietRooms = next;
      this.afterCarriedChange(before);
    }
    this.settleCoverWaiters();
  }

  /**
   * Listen to quiet rooms' events (thread summaries and metadata excluded).
   * Events carry their room's `h` tag.
   */
  listenQuietRooms(listener: RoomLiveListener) {
    this.quietListeners.add(listener);
    return () => {
      this.quietListeners.delete(listener);
    };
  }

  /** Called after every `reset()`, once the bundle is empty again. */
  onReset(listener: () => void) {
    this.resetListeners.add(listener);
    return () => {
      this.resetListeners.delete(listener);
    };
  }

  /**
   * Listen to room metadata (`ROOM_METADATA_KINDS`) of every room the bundle
   * carries, member or not. Events carry their room's `h` tag.
   */
  listenRoomMetadata(listener: RoomLiveListener) {
    this.metadataListeners.add(listener);
    return () => {
      this.metadataListeners.delete(listener);
    };
  }

  /**
   * Resolves once live events of `roomId` reach this bundle, so a history read
   * started afterwards misses nothing. Resolves at the first of: a chunk
   * carrying the room is live; membership is known and no chunk will carry
   * the room (archived, hidden, the huddle window, not a member: its live
   * events never come); `ROOM_COVERAGE_WAIT_MS`.
   */
  whenRoomCovered(roomId: string): Promise<void> {
    return this.whenRoomsCovered([roomId]).then(() => undefined);
  }

  /**
   * Like `whenRoomCovered`, for every room in `roomIds`: `true` once all are
   * covered, `false` when `ROOM_COVERAGE_WAIT_MS` passed first.
   */
  whenRoomsCovered(roomIds: Iterable<string>): Promise<boolean> {
    const ids = [...new Set(roomIds)];
    if (ids.every((roomId) => this.isRoomCovered(roomId))) {
      return Promise.resolve(true);
    }
    return new Promise<boolean>((resolve) => {
      const waiter: CoverWaiter = { roomIds: ids, resolve, timer: 0 };
      waiter.timer = this.timers.setTimeout(() => {
        this.coverWaiters.delete(waiter);
        resolve(false);
      }, ROOM_COVERAGE_WAIT_MS);
      this.coverWaiters.add(waiter);
    });
  }

  /** Listen to every member room's events (thread summaries excluded). */
  listenAllRooms(listener: RoomLiveListener) {
    this.allRoomListeners.add(listener);
    return () => {
      this.allRoomListeners.delete(listener);
    };
  }

  /** Look up the room of an event this bundle has not delivered itself. */
  setRoomResolver(resolve: (eventId: string) => string | null) {
    this.resolveRoomOfEvent = resolve;
    return () => {
      if (this.resolveRoomOfEvent === resolve) {
        this.resolveRoomOfEvent = () => null;
      }
    };
  }

  /**
   * Listen to one room, thread summaries included. Resolves once a live
   * subscription covers the room (or its attempt failed and is being
   * retried), with the function that stops listening.
   */
  async listenRoom(
    roomId: string,
    listener: RoomLiveListener,
  ): Promise<() => Promise<void>> {
    let listeners = this.roomListeners.get(roomId);
    if (!listeners) {
      listeners = new Set();
      this.roomListeners.set(roomId, listeners);
    }
    listeners.add(listener);
    this.extraRooms.set(roomId, (this.extraRooms.get(roomId) ?? 0) + 1);
    this.queueSync();

    let stopped = false;
    const stop = async () => {
      if (stopped) return;
      stopped = true;
      const current = this.roomListeners.get(roomId);
      current?.delete(listener);
      if (current?.size === 0) this.roomListeners.delete(roomId);
      const count = (this.extraRooms.get(roomId) ?? 1) - 1;
      if (count > 0) {
        this.extraRooms.set(roomId, count);
      } else {
        this.extraRooms.delete(roomId);
        this.queueSync();
        this.settleCoverWaiters();
      }
    };

    if (!this.isRoomSettled(roomId)) {
      await new Promise<void>((resolve) => {
        this.waiters.add({ roomId, resolve });
      });
    }
    return stop;
  }

  /** Every chunk delivery with its room, duplicates included (shadow comparison). */
  observeChunkEvents(observer: (event: RelayEvent, roomId: string) => void) {
    this.chunkObservers.add(observer);
    return () => {
      this.chunkObservers.delete(observer);
    };
  }

  /**
   * The user stream carries `rooms` from its live/gated EOSE (kept by
   * reference), or nothing (`null`): chunks then carry every room again.
   * Extras keep their own chunks (docs/message-sync.md#handoff).
   */
  setStreamRooms(rooms: ReadonlySet<string> | null, live = true) {
    if (this.streamRooms === rooms && this.streamLive === live) return;
    this.streamRooms = rooms;
    this.streamLive = live;
    this.queueSync();
    this.settleWaiters();
    this.settleCoverWaiters();
  }

  /** A stream room's set changed in place (44100/44101). */
  streamRoomsChanged() {
    this.queueSync();
    this.settleCoverWaiters();
  }

  /**
   * A live/gated stream frame of `roomId`, delivered once by id like a
   * chunk's. Returns whether the room is known here (member, quiet or open);
   * an unknown room asks for a list refresh.
   */
  dispatchStream(event: RelayEvent, roomId: string) {
    this.deliver(event, roomId);
    return (
      this.memberRooms.has(roomId) ||
      this.quietRooms.has(roomId) ||
      this.roomListeners.has(roomId)
    );
  }

  /** Whether an event id was delivered here (stream or chunk). */
  delivered(eventId: string) {
    return this.seen.has(eventId);
  }

  /** Member ∪ quiet rooms and the extras on their own chunks. */
  roomLists() {
    return {
      carried: new Set([...this.memberRooms, ...this.quietRooms]),
      extras: [...this.extraRooms.keys()],
    };
  }

  /** Rooms a chunk carries now (live or opening). */
  chunkRooms(): ReadonlySet<string> {
    const rooms = new Set<string>();
    for (const chunk of this.chunks.values()) {
      for (const roomId of chunk.rooms) rooms.add(roomId);
    }
    return rooms;
  }

  /** The room of a stream frame: its `h` tag or a referenced event's room. */
  streamRoomOf(event: RelayEvent) {
    const tagged = tagValue(event, "h");
    if (tagged) return tagged;
    for (const eventId of referencedIds(event)) {
      const roomId =
        this.eventRooms.get(eventId) ?? this.resolveRoomOfEvent(eventId);
      if (roomId) return roomId;
    }
    return null;
  }

  /** Drop every subscription and remembered event (community switch). */
  reset() {
    this.generation += 1;
    for (const chunk of this.chunks.values()) this.closeChunk(chunk);
    this.chunks.clear();
    this.memberRooms.clear();
    this.quietRooms.clear();
    for (const timer of this.graceRetained.values()) {
      this.timers.clearTimeout(timer);
    }
    this.graceRetained.clear();
    this.membershipKnown = false;
    this.streamRooms = null;
    this.streamLive = true;
    this.seen.clear();
    this.eventRooms.clear();
    // Rooms still listened to are covered again on the next session.
    this.queueSync();
    for (const listener of [...this.resetListeners]) listener();
  }

  /** Rooms packed into shared chunks: member ∪ quiet ∪ grace. */
  private carriedRooms() {
    return new Set([
      ...this.memberRooms,
      ...this.quietRooms,
      ...this.graceRetained.keys(),
    ]);
  }

  /**
   * Keeps rooms that left member ∪ quiet in their live chunk for
   * `ROOM_SHRINK_GRACE_MS`, so the chunk REQ is not re-sent if they come back
   * (hide then unhide, a refetch that briefly drops them). A room coming back
   * cancels its grace. No grace when another room joins in the same change
   * (a new REQ goes out anyway) or when the room's chunk is not live yet
   * (nothing to keep).
   */
  private afterCarriedChange(before: ReadonlySet<string>) {
    const now = new Set([...this.memberRooms, ...this.quietRooms]);
    const joined = [...now].some((roomId) => !before.has(roomId));
    for (const roomId of before) {
      if (
        joined ||
        now.has(roomId) ||
        this.graceRetained.has(roomId) ||
        !this.isRoomLive(roomId)
      ) {
        continue;
      }
      const timer = this.timers.setTimeout(() => {
        if (this.graceRetained.get(roomId) !== timer) return;
        this.graceRetained.delete(roomId);
        this.queueSync();
        this.settleCoverWaiters();
      }, ROOM_SHRINK_GRACE_MS);
      this.graceRetained.set(roomId, timer);
    }
    for (const [roomId, timer] of [...this.graceRetained]) {
      if (joined || now.has(roomId)) {
        this.timers.clearTimeout(timer);
        this.graceRetained.delete(roomId);
      }
    }
    this.queueSync();
  }

  private desiredChunks() {
    const carried = this.carriedRooms();
    const extras = [...this.extraRooms.keys()].filter(
      (roomId) => !carried.has(roomId),
    );
    const stream = this.streamRooms;
    const chunked = stream
      ? [...carried].filter((roomId) => !stream.has(roomId))
      : carried;
    return [
      ...chunkRoomIds(chunked),
      ...extras.sort().map((roomId) => [roomId]),
    ];
  }

  private queueSync() {
    if (this.syncQueued) return;
    this.syncQueued = true;
    queueMicrotask(() => {
      this.syncQueued = false;
      this.sync();
    });
  }

  private sync() {
    for (const rooms of this.desiredChunks()) {
      const key = rooms.join(",");
      if (!this.chunks.has(key)) this.openChunk(key, rooms);
    }
    this.retireReplacedChunks();
    this.settleWaiters();
  }

  private openChunk(key: string, rooms: string[]) {
    const chunk: Chunk = {
      key,
      rooms: new Set(rooms),
      status: "opening",
      failures: 0,
      dispose: null,
      retryTimer: null,
      controller: new AbortController(),
    };
    this.chunks.set(key, chunk);
    this.attempt(chunk);
  }

  private attempt(chunk: Chunk) {
    const generation = this.generation;
    const isCurrent = () =>
      generation === this.generation && this.chunks.get(chunk.key) === chunk;
    chunk.status = "opening";
    this.openSubscription(
      {
        kinds: [...ROOM_LIVE_KINDS],
        "#h": [...chunk.rooms],
        // No history and no `since`: live matching applies `since`, which
        // would drop events stamped up to the relay's skew allowance in the
        // past. Missed history comes from reconnect repair instead.
        limit: 0,
      },
      (event) => {
        if (isCurrent()) this.dispatch(chunk, event);
      },
      // The session retries a retryable CLOSED itself; a terminal one (access
      // revoked) changes membership, which opens a fresh chunk.
      () => {},
      chunk.controller.signal,
    ).then(
      (dispose) => {
        if (!isCurrent()) {
          void dispose().catch(() => {});
          return;
        }
        chunk.dispose = dispose;
        chunk.status = "live";
        chunk.failures = 0;
        this.retireReplacedChunks();
        this.settleWaiters();
        this.settleCoverWaiters();
      },
      (error: unknown) => {
        if (!isCurrent()) return;
        chunk.status = "failed";
        chunk.failures += 1;
        if (!(error instanceof DOMException && error.name === "AbortError")) {
          console.error("Failed to subscribe to live rooms", error);
        }
        this.settleWaiters();
        const delayMs = Math.min(
          RETRY_BASE_MS * 2 ** (chunk.failures - 1),
          RETRY_MAX_MS,
        );
        chunk.retryTimer = this.timers.setTimeout(() => {
          chunk.retryTimer = null;
          if (isCurrent() && this.isDesired(chunk.key)) this.attempt(chunk);
        }, delayMs);
      },
    );
  }

  private isDesired(key: string) {
    return this.desiredChunks().some((rooms) => rooms.join(",") === key);
  }

  /**
   * Close chunks no longer wanted, but only once every room they still carry
   * that is wanted is covered by a live wanted chunk.
   */
  private retireReplacedChunks() {
    const desired = this.desiredChunks();
    const desiredKeys = new Set(desired.map((rooms) => rooms.join(",")));
    const liveDesiredRooms = new Set<string>();
    for (const key of desiredKeys) {
      const chunk = this.chunks.get(key);
      if (chunk?.status !== "live") continue;
      for (const roomId of chunk.rooms) liveDesiredRooms.add(roomId);
    }
    const wantedRooms = new Set(desired.flat());
    for (const [key, chunk] of this.chunks) {
      if (desiredKeys.has(key)) continue;
      const stillNeeded = [...chunk.rooms].some(
        (roomId) => wantedRooms.has(roomId) && !liveDesiredRooms.has(roomId),
      );
      if (stillNeeded && chunk.status === "live") continue;
      this.chunks.delete(key);
      this.closeChunk(chunk);
    }
  }

  private closeChunk(chunk: Chunk) {
    chunk.controller.abort();
    if (chunk.retryTimer !== null) {
      this.timers.clearTimeout(chunk.retryTimer);
      chunk.retryTimer = null;
    }
    void chunk.dispose?.().catch(() => {});
  }

  /** A room is settled when a chunk carrying it is live or has given up once. */
  private isRoomSettled(roomId: string) {
    if (this.streamLive && this.streamRooms?.has(roomId)) return true;
    for (const chunk of this.chunks.values()) {
      if (!chunk.rooms.has(roomId)) continue;
      if (chunk.status !== "opening") return true;
    }
    return false;
  }

  private isRoomLive(roomId: string) {
    if (this.streamLive && this.streamRooms?.has(roomId)) return true;
    for (const chunk of this.chunks.values()) {
      if (chunk.status === "live" && chunk.rooms.has(roomId)) return true;
    }
    return false;
  }

  private isRoomCovered(roomId: string) {
    if (this.isRoomLive(roomId)) return true;
    return (
      this.membershipKnown &&
      !this.memberRooms.has(roomId) &&
      !this.quietRooms.has(roomId) &&
      !this.extraRooms.has(roomId)
    );
  }

  private settleCoverWaiters() {
    for (const waiter of [...this.coverWaiters]) {
      if (!waiter.roomIds.every((roomId) => this.isRoomCovered(roomId))) {
        continue;
      }
      this.coverWaiters.delete(waiter);
      this.timers.clearTimeout(waiter.timer);
      waiter.resolve(true);
    }
  }

  private settleWaiters() {
    for (const waiter of [...this.waiters]) {
      const stillWanted = this.extraRooms.has(waiter.roomId);
      if (!stillWanted || this.isRoomSettled(waiter.roomId)) {
        this.waiters.delete(waiter);
        waiter.resolve();
      }
    }
  }

  private roomOf(chunk: Chunk, event: RelayEvent) {
    const tagged = tagValue(event, "h");
    if (tagged) return tagged;
    if (chunk.rooms.size === 1)
      return chunk.rooms.values().next().value ?? null;
    for (const eventId of referencedIds(event)) {
      const roomId =
        this.eventRooms.get(eventId) ?? this.resolveRoomOfEvent(eventId);
      if (roomId && chunk.rooms.has(roomId)) return roomId;
    }
    return null;
  }

  private dispatch(chunk: Chunk, event: RelayEvent) {
    if (this.seen.has(event.id) && this.chunkObservers.size === 0) return;
    const roomId = this.roomOf(chunk, event);
    if (!roomId) return;
    for (const observer of this.chunkObservers) observer(event, roomId);
    this.deliver(event, roomId);
  }

  private deliver(event: RelayEvent, roomId: string) {
    if (this.seen.has(event.id)) return;
    remember(this.seen, event.id, true, SEEN_EVENT_LIMIT);
    remember(this.eventRooms, event.id, roomId, EVENT_ROOM_LIMIT);

    if (ROOM_METADATA_KINDS.includes(event.kind)) {
      const tagged = withRoomTag(event, roomId);
      for (const listener of [...this.metadataListeners]) listener(tagged);
      return;
    }
    const listeners = this.roomListeners.get(roomId);
    if (listeners) {
      for (const listener of [...listeners]) listener(event);
    }
    if (event.kind === KIND_CHANNEL_THREAD_SUMMARY) return;
    if (this.memberRooms.has(roomId)) {
      const tagged = withRoomTag(event, roomId);
      for (const listener of [...this.allRoomListeners]) listener(tagged);
    } else if (this.quietRooms.has(roomId)) {
      const tagged = withRoomTag(event, roomId);
      for (const listener of [...this.quietListeners]) listener(tagged);
    }
  }
}
