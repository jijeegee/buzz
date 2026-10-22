import {
  resolveKnownUserLabel,
  type UserProfileLookup,
} from "@/features/profile/lib/identity";
import type { Channel } from "@/shared/api/types";
import { normalizePubkey, UNKNOWN_USER_LABEL } from "@/shared/lib/pubkey";
import { t } from "@/shared/i18n";

/** Title of a chat with nobody else in it. */
export function emptyChatTitle(): string {
  return t("channels.chat.emptyTitle");
}
/** Names shown before the "+N" in an unnamed chat's title. */
export const CHAT_TITLE_NAME_LIMIT = 3;
/** Most people a new chat can start with (relay wire contract). */
export const MAX_NEW_CHAT_PEOPLE = 100;

type ChatShape = Pick<Channel, "channelType" | "groupChat">;

/** Ownerless multi-person chat (kind:39000 `["group_chat"]`). */
export function isGroupChat(channel: ChatShape | null | undefined): boolean {
  return channel?.groupChat === true;
}

/**
 * A personal conversation: a 1:1 DM or an ownerless chat. These notify and
 * count unread on every message, and live together in the Chats list.
 */
export function isChatRoom(channel: ChatShape | null | undefined): boolean {
  return channel?.channelType === "dm" || isGroupChat(channel);
}

/** Excluded from `#name` links, channel browsers and channel pickers. */
export function isListedAsChannel(channel: ChatShape): boolean {
  return channel.channelType !== "dm" && !isGroupChat(channel);
}

/** Everyone in the conversation (DM participants or chat members). */
export function chatPeoplePubkeys(
  channel: Pick<
    Channel,
    "channelType" | "groupChat" | "memberPubkeys" | "participantPubkeys"
  >,
): string[] {
  return isGroupChat(channel)
    ? channel.memberPubkeys
    : channel.participantPubkeys;
}

/** Distinct people other than `currentPubkey`, in relay order. */
export function otherChatPeople(
  pubkeys: readonly string[],
  currentPubkey: string | undefined,
): string[] {
  const me = currentPubkey ? normalizePubkey(currentPubkey) : null;
  const seen = new Set<string>();
  const out: string[] = [];
  for (const pubkey of pubkeys) {
    const key = normalizePubkey(pubkey);
    if (!key || key === me || seen.has(key)) continue;
    seen.add(key);
    out.push(key);
  }
  return out;
}

/** "Ann, Bob, Cy +2": first three names, then how many more. */
export function formatChatMemberTitle(
  labels: readonly (string | null)[],
): string {
  if (labels.length === 0) return emptyChatTitle();
  const shown = labels
    .slice(0, CHAT_TITLE_NAME_LIMIT)
    .map((label) => label?.trim() || UNKNOWN_USER_LABEL);
  const hidden = labels.length - shown.length;
  return hidden > 0 ? `${shown.join(", ")} +${hidden}` : shown.join(", ");
}

/**
 * The one title of an ownerless chat: your private name, else the shared
 * name, else the other members' names, else "Empty chat".
 */
export function resolveGroupChatTitle({
  channel,
  currentPubkey,
  privateName,
  profiles,
}: {
  channel: Pick<Channel, "name" | "memberPubkeys">;
  currentPubkey: string | undefined;
  privateName?: string | null;
  profiles: UserProfileLookup | undefined;
}): string {
  return groupChatTitleWithNames({
    channel,
    currentPubkey,
    privateName,
    nameOf: (pubkey) =>
      resolveKnownUserLabel({ currentPubkey, profiles, pubkey }),
  });
}

/**
 * {@link resolveGroupChatTitle} with a caller-supplied name lookup, for
 * non-React paths (desktop notifications) that read names from a cache.
 */
export function groupChatTitleWithNames({
  channel,
  currentPubkey,
  privateName,
  nameOf,
}: {
  channel: Pick<Channel, "name" | "memberPubkeys">;
  currentPubkey: string | undefined;
  privateName?: string | null;
  nameOf: (pubkey: string) => string | null;
}): string {
  const mine = privateName?.trim();
  if (mine) return mine;
  const shared = channel.name.trim();
  if (shared) return shared;
  return formatChatMemberTitle(
    otherChatPeople(channel.memberPubkeys, currentPubkey).map(nameOf),
  );
}

/** The people an invitation's chat picture shows (inviter if none listed). */
export function invitationChatPeople(invitation: {
  channel: { memberPubkeys?: string[] };
  inviter: string;
}): string[] {
  const listed = invitation.channel.memberPubkeys ?? [];
  return listed.length > 0 ? listed : [invitation.inviter];
}

/**
 * Title of a chat you're invited to: its name, else its members' names
 * ("+N" counts members not listed), else "Empty chat".
 */
export function invitationChatTitle(
  invitation: {
    channel: { name: string; memberCount: number; memberPubkeys?: string[] };
    inviter: string;
  },
  profiles: UserProfileLookup | undefined,
): string {
  const shared = invitation.channel.name.trim();
  if (shared) return shared;
  const people = invitationChatPeople(invitation);
  const labels = people.map((pubkey) =>
    resolveKnownUserLabel({ currentPubkey: undefined, profiles, pubkey }),
  );
  const title = formatChatMemberTitle(labels.slice(0, CHAT_TITLE_NAME_LIMIT));
  const hidden =
    Math.max(invitation.channel.memberCount, people.length) -
    Math.min(people.length, CHAT_TITLE_NAME_LIMIT);
  return hidden > 0 && labels.length > 0 ? `${title} +${hidden}` : title;
}

/** "1 person" / "N people". */
export function peopleCountLabel(count: number): string {
  return t("channels.chat.peopleCount", { count });
}

/** Toast after picked people became invitations instead of members. */
export function invitationsSentText(invitedCount: number, pickedCount: number) {
  if (invitedCount <= 0) return null;
  if (invitedCount === pickedCount && invitedCount > 1) {
    return t("channels.chat.invitationsSent");
  }
  return t("channels.chat.invitationSentTo", { count: invitedCount });
}
