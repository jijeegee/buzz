import * as React from "react";

import { useUsersBatchQuery } from "@/features/profile/hooks";
import { resolveUserLabel } from "@/features/profile/lib/identity";
import { useIdentityQuery } from "@/shared/api/hooks";
import type { Channel } from "@/shared/api/types";

import { chatPeoplePubkeys, otherChatPeople } from "../lib/groupChat";
import {
  ChatGridAvatar,
  type ChatGridAvatarSize,
  type ChatGridPerson,
} from "./ChatGridAvatar";

/** Grid tiles drawn at most. */
const GRID_PEOPLE = 4;

/** People drawn in a chat's picture: up to four others, or just you. */
export function chatGridPubkeys(
  pubkeys: readonly string[],
  currentPubkey: string | undefined,
): string[] {
  const others = otherChatPeople(pubkeys, currentPubkey).slice(0, GRID_PEOPLE);
  if (others.length > 0) return others;
  return currentPubkey ? [currentPubkey.toLowerCase()] : [];
}

/** {@link ChatGridAvatar} for a chat, from its people and their profiles. */
export function ChannelChatAvatar({
  channel,
  className,
  memberPubkeys,
  size,
  testId,
}: {
  channel?: Pick<
    Channel,
    "channelType" | "groupChat" | "memberPubkeys" | "participantPubkeys"
  >;
  className?: string;
  /** Explicit people (e.g. an invitation's preview) instead of `channel`. */
  memberPubkeys?: readonly string[];
  size?: ChatGridAvatarSize;
  testId?: string;
}) {
  const currentPubkey = useIdentityQuery().data?.pubkey;
  const pubkeys = React.useMemo(
    () =>
      chatGridPubkeys(
        memberPubkeys ?? (channel ? chatPeoplePubkeys(channel) : []),
        currentPubkey,
      ),
    [channel, currentPubkey, memberPubkeys],
  );
  const profiles = useUsersBatchQuery(pubkeys).data?.profiles;
  const people: ChatGridPerson[] = pubkeys.map((pubkey) => ({
    avatarUrl: profiles?.[pubkey]?.avatarUrl ?? null,
    isAgent: profiles?.[pubkey]?.isAgent === true,
    label: resolveUserLabel({
      currentPubkey: undefined,
      profiles,
      pubkey,
    }),
    pubkey,
  }));
  return (
    <ChatGridAvatar
      className={className}
      people={people}
      size={size}
      testId={testId}
    />
  );
}
