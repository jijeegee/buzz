import * as React from "react";

import { usePresenceQuery } from "@/features/presence/hooks";
import { resolveUserLabel } from "@/features/profile/lib/identity";
import { useUsersBatchQuery } from "@/features/profile/hooks";
import { resolveChannelDisplayLabel } from "@/features/sidebar/lib/channelLabels";
import type { SidebarDmParticipant } from "@/features/sidebar/ui/SidebarSection";
import type { Channel, PresenceStatus } from "@/shared/api/types";
import { useChatPrivateNames } from "@/features/channels/lib/chatPrivateNames";
import {
  CHAT_TITLE_NAME_LIMIT,
  otherChatPeople,
} from "@/features/channels/lib/groupChat";

export function useDmSidebarMetadata({
  currentPubkey,
  directMessages,
  fallbackDisplayName,
  profileDisplayName,
  enabled = true,
  groupChats,
}: {
  currentPubkey?: string;
  directMessages: Channel[];
  /** Ownerless chats: get titles (private name, shared name or members). */
  groupChats?: Channel[];
  fallbackDisplayName?: string;
  profileDisplayName?: string | null;
  enabled?: boolean;
}) {
  const selfDmLabels = React.useMemo(
    () =>
      new Set(
        [profileDisplayName, fallbackDisplayName]
          .map((value) => value?.trim().toLowerCase())
          .filter((value): value is string => Boolean(value)),
      ),
    [fallbackDisplayName, profileDisplayName],
  );
  const dmParticipantPubkeys = React.useMemo(
    () =>
      directMessages.flatMap((channel) =>
        channel.participantPubkeys.filter((pubkey, index) => {
          const normalizedPubkey = pubkey.toLowerCase();
          if (normalizedPubkey === currentPubkey?.toLowerCase()) {
            return false;
          }

          const participantLabel =
            channel.participants[index]?.trim().toLowerCase() ?? null;
          return !participantLabel || !selfDmLabels.has(participantLabel);
        }),
      ),
    [currentPubkey, directMessages, selfDmLabels],
  );
  // Ownerless chats ride along for titles only: the names a title shows.
  const chatTitlePubkeys = React.useMemo(
    () =>
      (groupChats ?? []).flatMap((channel) =>
        otherChatPeople(channel.memberPubkeys, currentPubkey).slice(
          0,
          CHAT_TITLE_NAME_LIMIT,
        ),
      ),
    [currentPubkey, groupChats],
  );
  const privateNames = useChatPrivateNames();
  const dmPresenceQuery = usePresenceQuery(dmParticipantPubkeys, {
    enabled: enabled && directMessages.length > 0,
  });
  const dmProfilesQuery = useUsersBatchQuery(
    [...dmParticipantPubkeys, ...chatTitlePubkeys],
    {
      enabled: enabled && directMessages.length + (groupChats?.length ?? 0) > 0,
    },
  );
  const dmProfiles = dmProfilesQuery.data?.profiles;
  const dmPresenceByChannelId = React.useMemo(
    () =>
      Object.fromEntries(
        directMessages.map((channel) => {
          const otherParticipantPubkey = channel.participantPubkeys.find(
            (pubkey, index) => {
              const normalizedPubkey = pubkey.toLowerCase();
              if (normalizedPubkey === currentPubkey?.toLowerCase()) {
                return false;
              }

              const participantLabel =
                channel.participants[index]?.trim().toLowerCase() ?? null;
              return !participantLabel || !selfDmLabels.has(participantLabel);
            },
          );

          return [
            channel.id,
            otherParticipantPubkey
              ? (dmPresenceQuery.data?.[otherParticipantPubkey.toLowerCase()] ??
                "offline")
              : "offline",
          ];
        }),
      ) satisfies Record<string, PresenceStatus>,
    [currentPubkey, directMessages, dmPresenceQuery.data, selfDmLabels],
  );
  const dmChannelLabels = React.useMemo(
    () =>
      Object.fromEntries(
        [...directMessages, ...(groupChats ?? [])].map((channel) => [
          channel.id,
          resolveChannelDisplayLabel(
            channel,
            currentPubkey,
            dmProfilesQuery.data?.profiles,
            privateNames.get(channel.id) ?? null,
          ),
        ]),
      ),
    [
      currentPubkey,
      directMessages,
      dmProfilesQuery.data,
      groupChats,
      privateNames,
    ],
  );
  const dmParticipantsByChannelId = React.useMemo(
    () =>
      Object.fromEntries(
        directMessages.map((channel) => {
          const participants = channel.participantPubkeys.map(
            (pubkey, index) => ({
              fallbackName: channel.participants[index] ?? null,
              pubkey,
            }),
          );
          const otherParticipants = participants.filter((participant) => {
            if (
              participant.pubkey.toLowerCase() === currentPubkey?.toLowerCase()
            ) {
              return false;
            }

            const participantLabel =
              participant.fallbackName?.trim().toLowerCase() ?? null;
            return !participantLabel || !selfDmLabels.has(participantLabel);
          });
          const visibleParticipants =
            otherParticipants.length > 0 ? otherParticipants : participants;

          return [
            channel.id,
            visibleParticipants.map((participant) => ({
              avatarUrl:
                dmProfiles?.[participant.pubkey.toLowerCase()]?.avatarUrl ??
                null,
              label: resolveUserLabel({
                currentPubkey,
                fallbackName: participant.fallbackName,
                profiles: dmProfiles,
                pubkey: participant.pubkey,
              }),
              ...(dmProfiles?.[participant.pubkey.toLowerCase()]?.isAgent ===
              true
                ? { isAgent: true }
                : {}),
              pubkey: participant.pubkey,
            })),
          ];
        }),
      ) satisfies Record<string, SidebarDmParticipant[]>,
    [currentPubkey, directMessages, dmProfiles, selfDmLabels],
  );

  return {
    dmChannelLabels,
    dmParticipantsByChannelId,
    dmPresenceByChannelId,
  };
}
