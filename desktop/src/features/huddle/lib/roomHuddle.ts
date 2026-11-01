import { relayClient } from "@/shared/api/relayClient";
import { RoomHuddleLifecycle } from "./roomHuddleLifecycle";

/**
 * Huddle lifecycle events per room on the room bundle
 * (`RoomHuddleLifecycle`), shared by the indicator, huddle cards and roster.
 */
export const roomHuddle = new RoomHuddleLifecycle({
  listenRoom: (roomId, onEvent) =>
    relayClient.subscribeToChannelLive(roomId, onEvent),
  fetchEvents: (filter) => relayClient.fetchEvents(filter),
});
