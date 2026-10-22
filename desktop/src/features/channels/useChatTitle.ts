import * as React from "react";

import { useUsersBatchQuery } from "@/features/profile/hooks";
import { useIdentityQuery } from "@/shared/api/hooks";
import type { Channel } from "@/shared/api/types";

import { useChatPrivateNames } from "./lib/chatPrivateNames";
import {
  CHAT_TITLE_NAME_LIMIT,
  isGroupChat,
  otherChatPeople,
  resolveGroupChatTitle,
} from "./lib/groupChat";

type TitledChat = Pick<
  Channel,
  "channelType" | "groupChat" | "id" | "memberPubkeys" | "name"
>;

/** Pubkeys whose names an unnamed chat's title needs (first three others). */
export function chatTitlePubkeys(
  channels: readonly TitledChat[],
  currentPubkey: string | undefined,
): string[] {
  const out = new Set<string>();
  for (const channel of channels) {
    if (!isGroupChat(channel)) continue;
    for (const pubkey of otherChatPeople(
      channel.memberPubkeys,
      currentPubkey,
    ).slice(0, CHAT_TITLE_NAME_LIMIT)) {
      out.add(pubkey);
    }
  }
  return [...out];
}

/**
 * Title resolver for a set of rooms: ownerless chats get the one chat title
 * (private name > shared name > members > "Empty chat"); every other room
 * keeps `fallback(channel)`.
 */
export function useChatTitles(channels: readonly TitledChat[]) {
  const currentPubkey = useIdentityQuery().data?.pubkey;
  const pubkeys = React.useMemo(
    () => chatTitlePubkeys(channels, currentPubkey),
    [channels, currentPubkey],
  );
  const profiles = useUsersBatchQuery(pubkeys, {
    enabled: pubkeys.length > 0,
  }).data?.profiles;
  const privateNames = useChatPrivateNames();
  return React.useCallback(
    (channel: TitledChat | null | undefined, fallback: string): string => {
      if (!channel || !isGroupChat(channel)) return fallback;
      return resolveGroupChatTitle({
        channel,
        currentPubkey,
        privateName: privateNames.get(channel.id) ?? null,
        profiles,
      });
    },
    [currentPubkey, privateNames, profiles],
  );
}

/** {@link useChatTitles} for one room. */
export function useChatTitle(
  channel: TitledChat | null | undefined,
  fallback: string,
): string {
  const channels = React.useMemo(() => (channel ? [channel] : []), [channel]);
  return useChatTitles(channels)(channel, fallback);
}
