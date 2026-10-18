import type { Channel } from "@/shared/api/types";

type DmShape = Pick<Channel, "channelType" | "participantPubkeys">;

function distinctParticipants(channel: DmShape): string[] {
  return [
    ...new Set(
      channel.participantPubkeys.map((pubkey) => pubkey.toLowerCase()),
    ),
  ];
}

/** A DM between me and one other person (or a note to myself). */
export function isOneToOneDm(channel: DmShape | null | undefined): boolean {
  return (
    channel?.channelType === "dm" && distinctParticipants(channel).length <= 2
  );
}

/**
 * Channels and group DMs fold a blocked author's messages to one line. A 1:1
 * DM does not: the server already leaves out what they sent while blocked,
 * and what they sent before the block shows as it was.
 */
export function foldsBlockedAuthors(
  channel: DmShape | null | undefined,
): boolean {
  return !isOneToOneDm(channel);
}

/** The other person in a 1:1 DM when I blocked them, otherwise null. */
export function blockedDmPeer(
  channel: DmShape | null | undefined,
  currentPubkey: string | null | undefined,
  isBlocked: (pubkey: string) => boolean,
): string | null {
  if (!channel || !currentPubkey || !isOneToOneDm(channel)) return null;
  const self = currentPubkey.toLowerCase();
  const peer = distinctParticipants(channel).find((pubkey) => pubkey !== self);
  return peer && isBlocked(peer) ? peer : null;
}
