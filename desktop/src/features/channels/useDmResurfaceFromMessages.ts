import * as React from "react";

import { huddleWindowChannelId } from "@/features/huddle/lib/huddleWindow";
import { relayClient } from "@/shared/api/relayClient";
import type { RelayEvent } from "@/shared/api/types";
import {
  getChannelMembers,
  type OpenDmInput,
} from "@/shared/api/tauriChannels";
import { relayEventChannelId } from "./dmResurface";
import { createHiddenDmResurfaceCoordinator } from "./hiddenDmResurfaceCoordinator";
import { resurfaceHiddenDmMessage } from "./hiddenDmResurfaceAction";
import { useHiddenDmIds } from "./useHiddenDmIds";

type UseDmResurfaceFromMessagesOptions = {
  pubkey: string | undefined;
  relayUrl: string | undefined;
  reopen: (input: OpenDmInput) => Promise<{ id: string }>;
};

/**
 * Resurfaces a hidden DM row the moment new activity lands in it.
 *
 * Hidden DMs ride the client's room bundle as quiet rooms
 * (`RoomLiveBundle.setQuietRooms`): the same `#h` chunks as the listed rooms,
 * no REQ of their own. Their events reach the quiet listener here and never
 * the all-rooms listeners, so nothing counts a hidden DM as unread until it is
 * shown again. Missed messages come back through the bundle's reconnect
 * repair, like any room's. Hiding never drops membership, so `#h` access
 * holds.
 *
 * A bundle reset (community switch) empties the quiet set; it is applied
 * again on reset. The huddle window carries no rooms and owns no resurface:
 * the main window does it.
 */
export function useDmResurfaceFromMessages({
  pubkey,
  relayUrl,
  reopen,
}: UseDmResurfaceFromMessagesOptions) {
  const hiddenDmIds = useHiddenDmIds(pubkey);
  const generationRef = React.useRef(0);
  const reopenLatest = React.useEffectEvent(reopen);

  // Stable dependency for the hidden-set membership, order-independent.
  const hiddenDmKey = React.useMemo(
    () => [...hiddenDmIds].sort().join(","),
    [hiddenDmIds],
  );

  React.useEffect(() => {
    const expectedSignerPubkey = pubkey?.trim().toLowerCase() ?? "";
    const expectedRelayUrl = relayUrl?.trim() ?? "";
    const channelIds = hiddenDmKey.length > 0 ? hiddenDmKey.split(",") : [];
    const generation = ++generationRef.current;
    if (
      !expectedSignerPubkey ||
      !expectedRelayUrl ||
      channelIds.length === 0 ||
      huddleWindowChannelId()
    ) {
      return;
    }

    const hiddenDmIdSet = new Set(channelIds);
    let disposed = false;
    const isCurrent = () => !disposed && generationRef.current === generation;

    // A coordinator owned by this generation: coalescing and cleanup touch only
    // its private map, so a torn-down generation's in-flight attempt can never
    // drop a follower coalesced onto the replacement listener.
    const coordinator = createHiddenDmResurfaceCoordinator({
      resurface: (event) =>
        resurfaceHiddenDmMessage({
          event,
          expectedRelayUrl,
          expectedSignerPubkey,
          hiddenDmIds: hiddenDmIdSet,
          fetchMembers: getChannelMembers,
          isCurrent,
          reopen: reopenLatest,
        }),
      isCurrent,
      onError: (channelId, error) => {
        if (isCurrent()) {
          console.error("Failed to resurface hidden DM", channelId, error);
        }
      },
    });

    const handleEvent = (event: RelayEvent) => {
      if (!isCurrent()) return;
      const channelId = relayEventChannelId(event);
      if (!channelId || !hiddenDmIdSet.has(channelId)) return;
      coordinator.handle(channelId, event);
    };

    const rooms = relayClient.liveRooms;
    const stopListening = rooms.listenQuietRooms(handleEvent);
    const applyQuietRooms = () => rooms.setQuietRooms(channelIds);
    applyQuietRooms();
    const stopReapplying = rooms.onReset(applyQuietRooms);

    return () => {
      disposed = true;
      generationRef.current += 1;
      stopReapplying();
      stopListening();
      rooms.setQuietRooms([]);
    };
  }, [pubkey, relayUrl, hiddenDmKey]);
}
