import {
  chatRowContextId,
  type ChatListPage,
  type ChatListRow,
} from "@/features/chats/chatListState";
import type { Channel } from "@/shared/api/types";

/**
 * Owner view: an agent's DMs and group chats that its owner is not in. The
 * relay lists them read-only (`chat_list` with `as_agent`), with no read
 * state; dots come from what the owner has opened on this device.
 */
export const AGENT_CHAT_LIST_QUERY_KEY = ["agent-chat-list"] as const;
export const AGENT_CHAT_LIST_PAGE_SIZE = 100;
/** Rows loaded per refresh; older chats stay out of the list. */
export const AGENT_CHAT_LIST_MAX_ROWS = 300;

export function agentChatListQueryKey(
  relayUrl: string,
  ownerPubkey: string,
  agentPubkey: string,
) {
  return [
    ...AGENT_CHAT_LIST_QUERY_KEY,
    relayUrl,
    ownerPubkey.toLowerCase(),
    agentPubkey.toLowerCase(),
  ] as const;
}

/** Load pages until `maxRows` rows or the end of the list. */
export async function loadAgentChatList(
  loadPage: (before: ChatListPage["next"]) => Promise<ChatListPage>,
  maxRows = AGENT_CHAT_LIST_MAX_ROWS,
): Promise<ChatListRow[]> {
  const rows: ChatListRow[] = [];
  const seen = new Set<string>();
  let before: ChatListPage["next"] = null;
  do {
    const page = await loadPage(before);
    for (const row of page.rows) {
      if (seen.has(row.key)) continue;
      seen.add(row.key);
      rows.push(row);
    }
    before = page.next;
  } while (before !== null && rows.length < maxRows);
  return rows;
}

/**
 * The chat a row belongs to, as a channel the owner can open read-only.
 * `participantPubkeys` puts the agent first, then the other people.
 */
export function channelFromAgentChatRow(
  row: ChatListRow,
  agentPubkey: string,
): Channel {
  const agent = agentPubkey.toLowerCase();
  const people = [
    agent,
    ...row.dm_members
      .map((pubkey) => pubkey.toLowerCase())
      .filter((pubkey) => pubkey !== agent),
  ];
  const isDm = row.channel_type === "dm";
  const latestAt = row.latest?.created_at ?? (row.at > 0 ? row.at : null);
  return {
    archivedAt: null,
    channelType: isDm ? "dm" : "stream",
    description: "",
    goalsEnabled: false,
    groupChat: isDm ? false : row.group_chat === true,
    id: row.channel_id,
    isMember: false,
    lastMessageAt:
      latestAt !== null ? new Date(latestAt * 1000).toISOString() : null,
    memberCount: row.member_count ?? people.length,
    memberPubkeys: people,
    name: row.channel_name,
    participantPubkeys: people,
    participants: people.map(() => ""),
    purpose: null,
    topic: null,
    ttlDeadline: null,
    ttlSeconds: null,
    visibility: "private",
  };
}

/** One room per chat: the channels behind an agent's list rows. */
export function agentChatChannels(
  rows: readonly ChatListRow[],
  agentPubkey: string,
): Channel[] {
  const byId = new Map<string, Channel>();
  const memberCounts = new Map<string, number>();
  for (const row of rows) {
    // DM thread rows carry no members; build the chat from the row that does.
    if ((memberCounts.get(row.channel_id) ?? -1) >= row.dm_members.length) {
      continue;
    }
    memberCounts.set(row.channel_id, row.dm_members.length);
    // Listed like a joined chat; the open chat itself stays read-only.
    byId.set(row.channel_id, {
      ...channelFromAgentChatRow(row, agentPubkey),
      isMember: true,
    });
  }
  return [...byId.values()];
}

/**
 * Rows with a dot on whatever moved since the owner last looked: the relay
 * sends no read state here, so `unread_mark` is set from `seenAt`. The
 * agent's own messages never add a dot.
 */
export function withSeenDots(
  rows: readonly ChatListRow[],
  agentPubkey: string,
  seenAt: (contextId: string) => number,
): ChatListRow[] {
  const agent = agentPubkey.toLowerCase();
  return rows.map((row) => {
    const latest = row.latest;
    const unread =
      latest !== null &&
      latest.pubkey.toLowerCase() !== agent &&
      latest.created_at > seenAt(chatRowContextId(row));
    return {
      ...row,
      read_seq: null,
      unread: 0,
      unread_mark: unread,
      unread_mentions: 0,
      unread_mentions_overflow: false,
      unread_overflow: false,
    };
  });
}

/** The row that stands for `channelId`: its room row, else a thread row. */
export function ownerViewChatRow(
  rows: readonly ChatListRow[],
  channelId: string,
): ChatListRow | null {
  const ofChannel = rows.filter((row) => row.channel_id === channelId);
  return ofChannel.find((row) => row.type === "room") ?? ofChannel[0] ?? null;
}
