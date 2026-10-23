import type { Channel, ChannelHistory } from "@/shared/api/types";

/**
 * Whether new members of a group see messages sent before they joined.
 * Groups default to hiding them: a missing kind:39000 `["history"]` tag (and
 * older relays) means `"since_join"`. DMs and group chats always hide them.
 */
export function newMembersSeeHistory(
  channel: Pick<Channel, "channelType" | "groupChat" | "history"> | null,
): boolean {
  if (!channel || channel.channelType === "dm" || channel.groupChat === true) {
    return false;
  }
  return channel.history === "all";
}

/** The kind:9002 `["history"]` value a switch position saves. */
export function historyForSwitch(checked: boolean): ChannelHistory {
  return checked ? "all" : "since_join";
}

/**
 * The switch exists only for groups (not DMs, not group chats), and only for
 * the owners and admins who may change it.
 */
export function canToggleChannelHistory(
  channel: Pick<Channel, "channelType" | "groupChat"> | null,
  canManageChannel: boolean,
): boolean {
  return (
    canManageChannel &&
    channel !== null &&
    channel.channelType !== "dm" &&
    channel.groupChat !== true
  );
}
