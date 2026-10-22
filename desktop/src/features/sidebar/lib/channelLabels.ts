import {
  resolveKnownUserLabel,
  type UserProfileLookup,
} from "@/features/profile/lib/identity";
import { formatDmParticipantLabels } from "@/features/channels/lib/dmParticipantDisplay";
import { getChatPrivateName } from "@/features/channels/lib/chatPrivateNames";
import {
  isGroupChat,
  resolveGroupChatTitle,
} from "@/features/channels/lib/groupChat";
import type { Channel } from "@/shared/api/types";
import { normalizePubkey } from "@/shared/lib/pubkey";

function isGenericDmChannelName(name: string) {
  const normalized = name.trim().toLowerCase();
  return (
    normalized.length === 0 ||
    normalized === "dm" ||
    normalized === "direct message" ||
    normalized === "direct messages" ||
    /^group dm\s*(\(\d+\))?$/.test(normalized)
  );
}

/**
 * The one title of a room wherever it shows. Ownerless chats resolve private
 * name > shared name > other members' names > "Empty chat"; DMs draw from
 * their participants; other rooms use their name.
 */
export function resolveChannelDisplayLabel(
  channel: Channel,
  currentPubkey: string | undefined,
  profiles: UserProfileLookup | undefined,
  privateName: string | null = getChatPrivateName(channel.id),
) {
  if (isGroupChat(channel)) {
    return resolveGroupChatTitle({
      channel,
      currentPubkey,
      privateName,
      profiles,
    });
  }
  if (channel.channelType !== "dm" || !isGenericDmChannelName(channel.name)) {
    return channel.name;
  }

  const participants = channel.participantPubkeys.map((pubkey, index) => ({
    fallbackName: channel.participants[index] ?? null,
    pubkey,
  }));
  const otherParticipants = currentPubkey
    ? participants.filter(
        (participant) =>
          participant.pubkey.toLowerCase() !== currentPubkey.toLowerCase(),
      )
    : participants;
  // Distinct people are distinct pubkeys; two people may share a label.
  const seenPubkeys = new Set<string>();
  const labels: (string | null)[] = [];
  for (const participant of otherParticipants.length > 0
    ? otherParticipants
    : participants) {
    const pubkey = normalizePubkey(participant.pubkey);
    if (seenPubkeys.has(pubkey)) continue;
    seenPubkeys.add(pubkey);
    labels.push(
      resolveKnownUserLabel({
        currentPubkey,
        fallbackName: participant.fallbackName,
        profiles,
        pubkey: participant.pubkey,
      }),
    );
  }

  return labels.length > 0 ? formatDmParticipantLabels(labels) : channel.name;
}
