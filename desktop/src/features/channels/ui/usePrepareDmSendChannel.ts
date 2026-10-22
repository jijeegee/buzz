import * as React from "react";
import { toast } from "sonner";

import {
  useCreateGroupChatMutation,
  useUpsertCachedChannel,
} from "@/features/channels/hooks";
import type { Channel } from "@/shared/api/types";
import { normalizePubkey } from "@/shared/lib/pubkey";

import { invitationsSentText, otherChatPeople } from "../lib/groupChat";

/** Mentioned people who are not already in the 1:1. */
export function peopleOutsideDm(
  participantPubkeys: readonly string[],
  mentionedPubkeys: readonly string[],
  currentPubkey: string | undefined,
): string[] {
  const inDm = new Set(participantPubkeys.map(normalizePubkey));
  return otherChatPeople(mentionedPubkeys, currentPubkey).filter(
    (pubkey) => !inDm.has(pubkey),
  );
}

/**
 * Send-channel preparation for a 1:1. A 1:1 never grows: mentioning someone
 * outside it asks "Start a new chat with X?"; on yes, a new chat with the
 * other person plus X is created first and the message goes there (the
 * caller navigates). On no (or failure) it resolves null and the draft
 * stays.
 */
export function usePrepareDmSendChannel(
  activeChannel: Channel | null,
  currentPubkey: string | undefined,
  confirmStartChat: (pubkeys: string[]) => Promise<boolean>,
) {
  const createGroupChat = useCreateGroupChatMutation();
  const upsertCachedChannel = useUpsertCachedChannel();

  return React.useCallback(
    async (additionalParticipantPubkeys: string[] = []) => {
      if (activeChannel?.channelType !== "dm") {
        return activeChannel?.id ?? null;
      }

      const outside = peopleOutsideDm(
        activeChannel.participantPubkeys,
        additionalParticipantPubkeys,
        currentPubkey,
      );
      if (outside.length === 0) {
        return activeChannel.id;
      }
      if (!(await confirmStartChat(outside))) {
        return null;
      }

      const people = [
        ...otherChatPeople(activeChannel.participantPubkeys, currentPubkey),
        ...outside,
      ];
      try {
        const result = await createGroupChat.mutateAsync({ pubkeys: people });
        await upsertCachedChannel(result.channel);
        const invited = invitationsSentText(
          result.invited.length,
          people.length,
        );
        if (invited) toast(invited);
        return result.channel.id;
      } catch (error) {
        toast.error(
          error instanceof Error ? error.message : "Could not start the chat.",
        );
        return null;
      }
    },
    [
      activeChannel,
      confirmStartChat,
      createGroupChat.mutateAsync,
      currentPubkey,
      upsertCachedChannel,
    ],
  );
}
