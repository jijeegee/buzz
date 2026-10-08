import {
  getInboxThreadRootId,
  type InboxItem,
} from "@/features/home/lib/inbox";

/** Who a DM room is shown as: the other participant, never the latest sender. */
export type InboxRoomPerson = {
  avatarUrl: string | null;
  isAgent: boolean;
  label: string;
  pubkey: string;
};

/**
 * The chat room an inbox row stands for in the Channels + Threads view.
 * The row renders only the room's identity (avatar + name); each room kind
 * decides where that identity comes from.
 */
export type InboxConversationRoom =
  | { kind: "dm"; title: string; person: InboxRoomPerson }
  | { kind: "channel"; channelId: string; channelLabel: string }
  | {
      kind: "thread";
      channelId: string;
      channelLabel: string;
      threadId: string;
    };

export type InboxRoomDmMetadata = {
  /** Sidebar DM room names, keyed by channel id. */
  dmChannelLabels?: Readonly<Record<string, string>>;
  /** Other participants of each DM, keyed by channel id. */
  dmParticipantsByChannelId?: Readonly<
    Record<
      string,
      readonly {
        avatarUrl: string | null;
        isAgent?: boolean;
        label: string;
        pubkey: string;
      }[]
    >
  >;
};

export function resolveInboxConversationRoom(
  item: InboxItem,
  {
    dmChannelLabels,
    dmParticipantsByChannelId,
    isAgentPubkey,
  }: InboxRoomDmMetadata & { isAgentPubkey: (pubkey: string) => boolean },
): InboxConversationRoom {
  const channelId = item.item.channelId ?? "";

  if (item.item.channelType === "dm") {
    const counterpart = dmParticipantsByChannelId?.[channelId]?.[0];
    // Without DM metadata yet, fall back to the sender so the row still renders.
    const person: InboxRoomPerson = counterpart
      ? {
          avatarUrl: counterpart.avatarUrl,
          isAgent:
            counterpart.isAgent === true || isAgentPubkey(counterpart.pubkey),
          label: counterpart.label,
          pubkey: counterpart.pubkey,
        }
      : {
          avatarUrl: item.avatarUrl,
          isAgent: isAgentPubkey(item.item.pubkey),
          label: item.senderLabel,
          pubkey: item.item.pubkey,
        };
    return {
      kind: "dm",
      person,
      title: dmChannelLabels?.[channelId] || person.label,
    };
  }

  const channelLabel = item.channelLabel?.trim() || "channel";
  const threadId = getInboxThreadRootId(item);
  return threadId
    ? { kind: "thread", channelId, channelLabel, threadId }
    : { kind: "channel", channelId, channelLabel };
}

/** Room name; thread rooms need the resolved thread name from the caller. */
export function formatInboxConversationRoomTitle(
  room: InboxConversationRoom,
  threadName?: string | null,
): string {
  switch (room.kind) {
    case "dm":
      return room.title;
    case "channel":
      return `#${room.channelLabel}`;
    case "thread":
      return `#${room.channelLabel} › ${threadName || "Thread"}`;
  }
}
