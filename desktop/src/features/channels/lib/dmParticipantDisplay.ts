import {
  resolveKnownUserLabel,
  resolveUserLabel,
  type UserProfileLookup,
} from "@/features/profile/lib/identity";
import type { Channel } from "@/shared/api/types";
import { normalizePubkey, UNKNOWN_USER_LABEL } from "@/shared/lib/pubkey";

export const DM_PARTICIPANT_PREVIEW_LIMIT = 3;

export type DirectMessageIntroParticipant = {
  avatarUrl: string | null;
  displayName: string;
  isAgent?: boolean;
  pubkey: string;
};

export type DirectMessageIntro = {
  displayName: string;
  participants: DirectMessageIntroParticipant[];
};

export function getDmParticipantPreview<T>(participants: readonly T[]) {
  const visibleParticipants = participants.slice(
    0,
    DM_PARTICIPANT_PREVIEW_LIMIT,
  );

  return {
    hiddenCount: Math.max(
      0,
      participants.length - DM_PARTICIPANT_PREVIEW_LIMIT,
    ),
    visibleParticipants,
  };
}

/**
 * Join one label per distinct person (`null` = no known name). Unnamed people
 * are indistinguishable by label, so they share one entry at the first one's
 * position, counted ("Unknown user (2)") rather than repeated; `+N more`
 * counts hidden people, not hidden entries.
 */
export function formatDmParticipantLabels(
  labels: readonly (string | null)[],
): string {
  const entries: { label: string; count: number }[] = [];
  let unnamed: { label: string; count: number } | null = null;
  for (const label of labels) {
    if (label) {
      entries.push({ label, count: 1 });
    } else if (unnamed) {
      unnamed.count += 1;
    } else {
      unnamed = { label: UNKNOWN_USER_LABEL, count: 1 };
      entries.push(unnamed);
    }
  }
  const visible = entries.slice(0, DM_PARTICIPANT_PREVIEW_LIMIT);
  const hiddenCount = entries
    .slice(DM_PARTICIPANT_PREVIEW_LIMIT)
    .reduce((total, entry) => total + entry.count, 0);
  const names = visible.map(({ label, count }) =>
    count > 1 ? `${label} (${count})` : label,
  );
  return hiddenCount > 0
    ? [...names, `+${hiddenCount} more`].join(", ")
    : names.join(", ");
}

export function buildDirectMessageIntro({
  channel,
  currentPubkey,
  profiles,
}: {
  channel: Channel | null;
  currentPubkey?: string;
  profiles?: UserProfileLookup;
}): DirectMessageIntro | null {
  if (channel?.channelType !== "dm") {
    return null;
  }

  const participants = channel.participantPubkeys.map((pubkey, index) => ({
    fallbackName: channel.participants[index] ?? null,
    pubkey,
  }));
  const normalizedCurrentPubkey = currentPubkey
    ? normalizePubkey(currentPubkey)
    : null;
  const otherParticipants = normalizedCurrentPubkey
    ? participants.filter(
        (participant) =>
          normalizePubkey(participant.pubkey) !== normalizedCurrentPubkey,
      )
    : participants;
  // Distinct people are distinct pubkeys; drop repeats.
  const seenPubkeys = new Set<string>();
  const displayParticipants = (
    otherParticipants.length > 0 ? otherParticipants : participants
  ).filter((participant) => {
    const pubkey = normalizePubkey(participant.pubkey);
    if (seenPubkeys.has(pubkey)) return false;
    seenPubkeys.add(pubkey);
    return true;
  });

  if (displayParticipants.length === 0) {
    return null;
  }

  const introParticipants = displayParticipants.map((participant) => {
    const profile = profiles?.[normalizePubkey(participant.pubkey)] ?? null;

    return {
      avatarUrl: profile?.avatarUrl ?? null,
      displayName: resolveUserLabel({
        currentPubkey,
        fallbackName: participant.fallbackName,
        profiles,
        pubkey: participant.pubkey,
      }),
      ...(profile?.isAgent === true ? { isAgent: true } : {}),
      pubkey: participant.pubkey,
    };
  });

  return {
    displayName: formatDmParticipantLabels(
      displayParticipants.map((participant) =>
        resolveKnownUserLabel({
          currentPubkey,
          fallbackName: participant.fallbackName,
          profiles,
          pubkey: participant.pubkey,
        }),
      ),
    ),
    participants: introParticipants,
  };
}
