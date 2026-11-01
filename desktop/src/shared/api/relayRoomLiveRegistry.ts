import type { RelaySubscriptionFilter } from "@/shared/api/relayClientShared";
import type { RelayEvent } from "@/shared/api/types";
import { KIND_TYPING_INDICATOR } from "@/shared/constants/kinds";

/** Goal tree heads (kuzz-core `KIND_GOAL_TREE`). */
export const KIND_GOAL_TREE = 40110;

/**
 * The kinds an open room's own live REQ carries, one filter each, always
 * all of them: a listener coming or going never re-sends the REQ. Shared with
 * mobile through `test-fixtures/room-live-request.json`
 * (docs/message-sync.md#open-room-req).
 */
export const ROOM_LIVE_REQUEST_KINDS: readonly number[] = [
  KIND_TYPING_INDICATOR,
  KIND_GOAL_TREE,
];

/** The open-room REQ's filters for `roomId`: live only, one per kind. */
export function roomLiveRequestFilters(
  roomId: string,
): RelaySubscriptionFilter[] {
  return ROOM_LIVE_REQUEST_KINDS.map((kind) => ({
    kinds: [kind],
    "#h": [roomId],
    limit: 0,
  }));
}

export type RoomLiveOptions = {
  /** The session reconnected: events may have been missed; read again. */
  onResync?: () => void;
  /** The room's REQ was dropped for good (terminal CLOSED); it is not reopened. */
  onGone?: () => void;
};

export type OpenRoomLiveRequest = (
  filters: RelaySubscriptionFilter[],
  onEvent: (event: RelayEvent) => void,
  onGone: () => void,
) => Promise<() => Promise<void>>;

type Listener = RoomLiveOptions & {
  kind: number;
  onEvent: (event: RelayEvent) => void;
};

type Entry = {
  roomId: string;
  listeners: Set<Listener>;
  dispose: (() => Promise<void>) | null;
  closing: boolean;
  gone: boolean;
};

/**
 * One live REQ per open room for the room's typing indicators and goal tree,
 * shared by every listener of that room (`ROOM_LIVE_REQUEST_KINDS`).
 *
 * - The first listener opens it; it closes one microtask after the last
 *   listener leaves, so a remount in the same task keeps it.
 * - `listen` returns its stop function at once: stopping before the REQ is
 *   up is safe.
 * - A terminal CLOSED calls every listener's `onGone` and forgets the room,
 *   so a later listener (e.g. after rejoining) opens a fresh REQ.
 * - `resync` (on reconnect) calls `onResync` once per listener.
 */
export class RoomLiveRegistry {
  private entries = new Map<string, Entry>();
  private readonly open: OpenRoomLiveRequest;
  private readonly defer: (fn: () => void) => void;

  constructor(
    open: OpenRoomLiveRequest,
    defer: (fn: () => void) => void = queueMicrotask,
  ) {
    this.open = open;
    this.defer = defer;
  }

  listen(
    roomId: string,
    kind: number,
    onEvent: (event: RelayEvent) => void,
    options: RoomLiveOptions = {},
  ): () => void {
    if (!ROOM_LIVE_REQUEST_KINDS.includes(kind)) {
      throw new Error(`Kind ${kind} does not ride the open-room REQ.`);
    }
    let entry = this.entries.get(roomId);
    if (!entry) {
      entry = this.openEntry(roomId);
    }
    const listener: Listener = { ...options, kind, onEvent };
    entry.listeners.add(listener);
    entry.closing = false;
    const owner = entry;
    let stopped = false;
    return () => {
      if (stopped) return;
      stopped = true;
      owner.listeners.delete(listener);
      if (owner.listeners.size > 0 || owner.closing) return;
      owner.closing = true;
      this.defer(() => {
        if (!owner.closing || owner.listeners.size > 0) return;
        this.closeEntry(owner);
      });
    };
  }

  /** The session reconnected: every listener reads what it may have missed. */
  resync() {
    for (const entry of this.entries.values()) {
      for (const listener of [...entry.listeners]) listener.onResync?.();
    }
  }

  /** Rooms with a REQ open or opening (for tests). */
  openRooms(): string[] {
    return [...this.entries.keys()];
  }

  private openEntry(roomId: string): Entry {
    const entry: Entry = {
      roomId,
      listeners: new Set(),
      dispose: null,
      closing: false,
      gone: false,
    };
    this.entries.set(roomId, entry);
    const onGone = () => {
      if (entry.gone) return;
      entry.gone = true;
      if (this.entries.get(roomId) === entry) this.entries.delete(roomId);
      for (const listener of [...entry.listeners]) listener.onGone?.();
    };
    this.open(
      roomLiveRequestFilters(roomId),
      (event) => {
        if (entry.gone) return;
        for (const listener of [...entry.listeners]) {
          if (listener.kind === event.kind) listener.onEvent(event);
        }
      },
      onGone,
    ).then(
      (dispose) => {
        if (this.entries.get(roomId) !== entry) {
          void dispose().catch(() => {});
          return;
        }
        entry.dispose = dispose;
      },
      (error: unknown) => {
        if (!(error instanceof DOMException && error.name === "AbortError")) {
          console.error("Failed to open the room's live REQ", error);
        }
        onGone();
      },
    );
    return entry;
  }

  private closeEntry(entry: Entry) {
    if (this.entries.get(entry.roomId) === entry) {
      this.entries.delete(entry.roomId);
    }
    entry.gone = true;
    void entry.dispose?.().catch(() => {});
    entry.dispose = null;
  }
}
