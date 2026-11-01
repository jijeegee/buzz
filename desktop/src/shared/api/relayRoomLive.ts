import { relayClient } from "@/shared/api/relayClient";
import {
  RoomLiveRegistry,
  type RoomLiveOptions,
} from "@/shared/api/relayRoomLiveRegistry";
import type { RelayEvent } from "@/shared/api/types";

const registry = new RoomLiveRegistry((filters, onEvent, onGone) =>
  relayClient.subscribeRoomLiveFilters(
    filters,
    onEvent,
    undefined,
    undefined,
    onGone,
  ),
);

// Watched for the app's lifetime, from the first listener on.
let watchingReconnects = false;

/**
 * The open room's own live REQ (`RoomLiveRegistry`): typing indicators and
 * the goal tree of one room on one REQ, shared by every screen of that room.
 * An object so tests can stub it.
 */
export const roomLive = {
  /**
   * Listen to `kind` events of `roomId`. Returns the function that stops
   * listening, at once; safe to call before the REQ is up.
   */
  listen(
    roomId: string,
    kind: number,
    onEvent: (event: RelayEvent) => void,
    options?: RoomLiveOptions,
  ): () => void {
    if (!watchingReconnects) {
      watchingReconnects = true;
      relayClient.subscribeToReconnects(() => registry.resync());
    }
    return registry.listen(roomId, kind, onEvent, options);
  },
};
