import {
  resolveKnownUserLabel,
  type UserProfileLookup,
} from "@/features/profile/lib/identity";
import { formatDmParticipantLabels } from "@/features/channels/lib/dmParticipantDisplay";
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

export function resolveChannelDisplayLabel(
  channel: Channel,
  currentPubkey: string | undefined,
  profiles: UserProfileLookup | undefined,
) {
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
