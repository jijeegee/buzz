import { invokeTauri } from "@/shared/api/tauri";
import type { RelayEvent } from "@/shared/api/types";

/** `commands/channels_catch_up.rs` `CatchUpResult`. */
export type ChannelCatchUpResult = {
  status: "caughtUp" | "fresh" | "stale" | "unavailable";
  /** Every non-gap room was caught up from here (only when `caughtUp`). */
  since: number | null;
  /** Caught-up events of the dispatch rooms. */
  events: RelayEvent[];
  gapChannelIds: string[];
};

/**
 * Catch every given room up from the scope's watermark for socket `wsId`, in
 * one paged query per 128 rooms. `dispatchChannelIds` (also caught up) are
 * the rooms whose events come back.
 */
export function channelsCatchUp(
  wsId: number,
  channelIds: readonly string[],
  dispatchChannelIds: readonly string[],
): Promise<ChannelCatchUpResult> {
  return invokeTauri<ChannelCatchUpResult>("channels_catch_up", {
    wsId,
    channelIds,
    dispatchChannelIds,
  });
}

/**
 * Joined rooms of the active community's channel list, by relay origin, so a
 * reconnect catches every joined room up, not only rooms with a live
 * subscription. Reset with the community (`resetCommunityState`).
 */
let joinedRooms: { relay: string; ids: readonly string[] } | null = null;

/** The server's host, so its `wss://` and `https://` forms match. */
const relayKey = (relayUrl: string) => {
  try {
    return new URL(relayUrl.trim()).host.toLowerCase();
  } catch {
    return relayUrl.trim().toLowerCase();
  }
};

export function publishJoinedRooms(
  relayUrl: string,
  channels: readonly {
    id: string;
    isMember: boolean;
    archivedAt: string | null;
  }[],
) {
  joinedRooms = {
    relay: relayKey(relayUrl),
    ids: channels
      .filter((channel) => channel.isMember && channel.archivedAt === null)
      .map((channel) => channel.id),
  };
}

export function joinedRoomsFor(relayUrl: string | null): readonly string[] {
  if (!relayUrl || joinedRooms?.relay !== relayKey(relayUrl)) return [];
  return joinedRooms.ids;
}

/** The session's catch-up for its open socket: joined rooms plus `dispatch`. */
export async function catchUpSocket(
  wsId: number | null,
  relayUrl: string | null,
  dispatchChannelIds: string[],
): Promise<ChannelCatchUpResult | null> {
  if (wsId === null) return null;
  return channelsCatchUp(wsId, joinedRoomsFor(relayUrl), dispatchChannelIds);
}

export function resetJoinedRooms() {
  joinedRooms = null;
}
