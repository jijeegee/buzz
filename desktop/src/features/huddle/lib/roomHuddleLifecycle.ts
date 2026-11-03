import {
  type HistoryThenLive,
  historyThenLive,
} from "@/shared/api/historyThenLive";
import type { RelaySubscriptionFilter } from "@/shared/api/relayClientShared";
import type { RelayEvent } from "@/shared/api/types";
import {
  KIND_HUDDLE_ENDED,
  KIND_HUDDLE_PARTICIPANT_JOINED,
  KIND_HUDDLE_PARTICIPANT_LEFT,
  KIND_HUDDLE_STARTED,
} from "@/shared/constants/kinds";

/** Huddle lifecycle kinds (start, join, leave, end). */
export const HUDDLE_LIFECYCLE_KINDS: readonly number[] = [
  KIND_HUDDLE_STARTED,
  KIND_HUDDLE_PARTICIPANT_JOINED,
  KIND_HUDDLE_PARTICIPANT_LEFT,
  KIND_HUDDLE_ENDED,
];

/** Recent lifecycle events read once per room: long huddles join and leave a lot. */
export const HUDDLE_SEED_LIMIT = 100;

/** Events kept per room; older ones never change who is in a huddle now. */
const ROOM_EVENT_LIMIT = 500;

export type RoomHuddleDeps = {
  /** The room's live events on the room bundle; resolves once covered. */
  listenRoom: (
    roomId: string,
    onEvent: (event: RelayEvent) => void,
  ) => Promise<() => Promise<void>>;
  /** One history read. */
  fetchEvents: (filter: RelaySubscriptionFilter) => Promise<RelayEvent[]>;
  defer?: (fn: () => void) => void;
};

type RoomEntry = {
  roomId: string;
  events: Map<string, RelayEvent>;
  listeners: Set<(event: RelayEvent) => void>;
  live: HistoryThenLive | null;
  closing: boolean;
  closed: boolean;
};

/**
 * Huddle lifecycle events per room, shared by every huddle view of the room
 * (indicator, cards, roster) instead of one REQ each.
 *
 * The first listener listens to the room on the room bundle (which carries
 * these kinds and repairs them after a reconnect), then reads the recent
 * history once (`historyThenLive`). Every listener gets each event once:
 * all known events when it starts listening, then each new one. The room
 * stops listening one microtask after its last listener leaves.
 */
export class RoomHuddleLifecycle {
  private rooms = new Map<string, RoomEntry>();
  private readonly deps: RoomHuddleDeps;
  private readonly defer: (fn: () => void) => void;

  constructor(deps: RoomHuddleDeps) {
    this.deps = deps;
    // Wrapped: browsers throw "Illegal invocation" for a detached queueMicrotask.
    this.defer = deps.defer ?? ((fn) => queueMicrotask(fn));
  }

  /** Returns the function that stops listening, at once. */
  listen(roomId: string, onEvent: (event: RelayEvent) => void): () => void {
    let entry = this.rooms.get(roomId);
    if (!entry) entry = this.start(roomId);
    entry.closing = false;
    entry.listeners.add(onEvent);
    for (const event of [...entry.events.values()]) onEvent(event);
    const owner = entry;
    let stopped = false;
    return () => {
      if (stopped) return;
      stopped = true;
      owner.listeners.delete(onEvent);
      if (owner.listeners.size > 0 || owner.closing) return;
      owner.closing = true;
      this.defer(() => {
        if (!owner.closing || owner.listeners.size > 0) return;
        this.stop(owner);
      });
    };
  }

  private start(roomId: string): RoomEntry {
    const entry: RoomEntry = {
      roomId,
      events: new Map(),
      listeners: new Set(),
      live: null,
      closing: false,
      closed: false,
    };
    this.rooms.set(roomId, entry);
    const apply = (event: RelayEvent) => {
      if (entry.closed || entry.events.has(event.id)) return;
      entry.events.set(event.id, event);
      if (entry.events.size > ROOM_EVENT_LIMIT) {
        const oldest = [...entry.events.values()].sort(
          (a, b) => a.created_at - b.created_at,
        )[0];
        if (oldest) entry.events.delete(oldest.id);
      }
      for (const listener of [...entry.listeners]) listener(event);
    };
    historyThenLive({
      listen: (gate) =>
        this.deps.listenRoom(roomId, (event) => {
          if (HUDDLE_LIFECYCLE_KINDS.includes(event.kind)) gate(event);
        }),
      fetch: () =>
        this.deps.fetchEvents({
          kinds: [...HUDDLE_LIFECYCLE_KINDS],
          "#h": [roomId],
          limit: HUDDLE_SEED_LIMIT,
        }),
      apply,
      onFetchError: (error) => {
        console.error("[huddle] lifecycle history failed:", roomId, error);
      },
    }).then(
      (live) => {
        if (entry.closed) {
          void live.stop();
          return;
        }
        entry.live = live;
      },
      (error: unknown) => {
        console.error("[huddle] lifecycle listen failed:", roomId, error);
        if (this.rooms.get(roomId) === entry) this.rooms.delete(roomId);
      },
    );
    return entry;
  }

  private stop(entry: RoomEntry) {
    entry.closed = true;
    if (this.rooms.get(entry.roomId) === entry) this.rooms.delete(entry.roomId);
    void entry.live?.stop();
    entry.live = null;
  }
}
