import type { RelaySubscriptionFilter } from "@/shared/api/relayClientShared";
import { chunkRoomIds } from "@/shared/api/relayRoomBundle";
import type { RelayEvent } from "@/shared/api/types";
import {
  KIND_HUDDLE_ENDED,
  KIND_HUDDLE_PARTICIPANT_JOINED,
  KIND_HUDDLE_PARTICIPANT_LEFT,
  KIND_HUDDLE_STARTED,
} from "@/shared/constants/kinds";

export const HUDDLE_LIFECYCLE_KINDS = [
  KIND_HUDDLE_STARTED,
  KIND_HUDDLE_PARTICIPANT_JOINED,
  KIND_HUDDLE_PARTICIPANT_LEFT,
  KIND_HUDDLE_ENDED,
] as const;

type Dispose = () => void | Promise<void>;
type Listener = (event: RelayEvent) => void;

/** Where the presence runtime hears lifecycle events live. */
export type HuddlePresenceSource = {
  /** Attaches `onEvent`; resolves once attached. */
  listenLive: (onEvent: Listener) => Promise<Dispose>;
  /** `true` once every room is live; `false` when the wait's bound passed
   *  first (the runtime reconciles, then waits again). */
  whenLive: () => Promise<boolean>;
};

const LIFECYCLE_KIND_SET = new Set<number>(HUDDLE_LIFECYCLE_KINDS);

function roomOf(event: RelayEvent) {
  return event.tags.find((tag) => tag[0] === "h")?.[1] ?? null;
}

/**
 * Main window: lifecycle events from the room bundle's listed rooms, no REQ
 * of its own. Missed events come back through the bundle's reconnect repair
 * (lifecycle kinds are repair kinds) and the runtime's reconcile.
 */
export function roomBundlePresenceSource({
  channelIds,
  listenAllRooms,
  whenRoomsCovered,
}: {
  channelIds: readonly string[];
  listenAllRooms: (listener: Listener) => () => void;
  whenRoomsCovered: (roomIds: readonly string[]) => Promise<boolean>;
}): HuddlePresenceSource {
  const rooms = new Set(channelIds);
  return {
    listenLive: async (onEvent) =>
      listenAllRooms((event) => {
        if (!LIFECYCLE_KIND_SET.has(event.kind)) return;
        const roomId = roomOf(event);
        if (roomId && rooms.has(roomId)) onEvent(event);
      }),
    whenLive: () => whenRoomsCovered([...rooms]),
  };
}

/**
 * Huddle window: it carries no rooms, so it keeps its own lifecycle REQ per
 * 128 rooms. `limit: 0` with no `since`: history comes from the runtime's
 * reconcile, and a re-sent REQ is byte-identical.
 */
export function huddleWindowPresenceSource({
  channelIds,
  subscribeLive,
}: {
  channelIds: readonly string[];
  subscribeLive: (
    filter: RelaySubscriptionFilter,
    onEvent: Listener,
  ) => Promise<Dispose>;
}): HuddlePresenceSource {
  return {
    listenLive: async (onEvent) => {
      const disposes: Dispose[] = [];
      try {
        for (const chunk of chunkRoomIds(channelIds)) {
          disposes.push(
            await subscribeLive(
              { kinds: [...HUDDLE_LIFECYCLE_KINDS], "#h": chunk, limit: 0 },
              onEvent,
            ),
          );
        }
      } catch (error) {
        await Promise.all(disposes.map((dispose) => dispose()));
        throw error;
      }
      return () =>
        Promise.all(disposes.map((dispose) => dispose())).then(() => {});
    },
    whenLive: async () => true,
  };
}
