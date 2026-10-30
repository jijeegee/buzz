/**
 * Opening a room from its Chats row (docs/message-sync.md#launch-snapshots):
 * a room the sidebar channel list does not hold yet (the list still loading
 * at launch, or behind a new DM, a notification, a deep link or a search
 * result) opens at once from what its Chats row says (name, kind, people)
 * instead of waiting on a loading screen; the store paint then draws its
 * messages. The rows are the ones the Chats list shows: the relay's once it
 * answered this run, until then the ones kept from the last run.
 *
 * Such a channel only stands in for display until the real one arrives: it
 * names no members (so nothing routes to, mentions or marks agents from
 * it), and the pane takes no sends while it is shown (isChatRowStandIn).
 */
import type { QueryClient } from "@tanstack/react-query";
import { useQueryClient } from "@tanstack/react-query";
import * as React from "react";

import { formatDmParticipantLabels } from "@/features/channels/lib/dmParticipantDisplay";
import type { ChatListRow } from "@/features/chats/chatListState";
import { resolveKnownUserLabel } from "@/features/profile/lib/identity";
import {
  getUserLabelsVersion,
  readCachedUserLabels,
  subscribeUserLabels,
  type UserLabelScope,
} from "@/features/profile/lib/userLabelStorage";
import type { Channel } from "@/shared/api/types";
import { truncateNpub } from "@/shared/lib/pubkey";

const standIns = new WeakSet<Channel>();

/** Whether `channel` is a Chats row's display stand-in (see above). */
export function isChatRowStandIn(channel: Channel | null | undefined) {
  return channel ? standIns.has(channel) : false;
}

type Shown = { rows: readonly ChatListRow[]; listeners: Set<() => void> };

const shownByClient = new WeakMap<QueryClient, Shown>();

function shownIn(queryClient: QueryClient): Shown {
  let shown = shownByClient.get(queryClient);
  if (!shown) {
    shown = { rows: [], listeners: new Set() };
    shownByClient.set(queryClient, shown);
  }
  return shown;
}

/** The Chats rows now shown for `queryClient`'s account and server. */
export function publishShownChatRows(
  queryClient: QueryClient,
  rows: readonly ChatListRow[] | undefined,
) {
  const shown = shownIn(queryClient);
  const next = rows ?? [];
  if (shown.rows === next) return;
  shown.rows = next;
  for (const listener of shown.listeners) listener();
}

/**
 * The row that best describes room `channelId`: its room row, else the
 * thread row naming the most people (DM thread rows carry none).
 */
export function chatRowForRoom(
  rows: readonly ChatListRow[],
  channelId: string,
): ChatListRow | null {
  let best: ChatListRow | null = null;
  for (const row of rows) {
    if (row.channel_id !== channelId) continue;
    if (row.type === "room") return row;
    if (!best || row.dm_members.length > best.dm_members.length) best = row;
  }
  return best;
}

/**
 * The room a Chats row describes, as a display stand-in the account is a
 * member of (the relay lists only rooms the account belongs to). It names
 * no members; a DM, or a group chat without a name, is titled by the row's
 * other people as `labelOf` names them (kept profile names), else by a
 * short id.
 */
export function channelFromChatRow(
  row: ChatListRow,
  selfPubkey: string | null,
  labelOf: (pubkey: string) => string | null = () => null,
): Channel {
  const self = selfPubkey?.toLowerCase() ?? null;
  const others = [
    ...new Set(row.dm_members.map((pubkey) => pubkey.toLowerCase())),
  ].filter((pubkey) => pubkey !== self);
  const channelType =
    row.channel_type === "dm"
      ? "dm"
      : row.channel_type === "forum"
        ? "forum"
        : "stream";
  const groupChat = channelType === "dm" ? false : row.group_chat === true;
  const titledByPeople =
    others.length > 0 &&
    (channelType === "dm" || (groupChat && !row.channel_name.trim()));
  const latestAt = row.latest?.created_at ?? (row.at > 0 ? row.at : null);
  const channel: Channel = {
    archivedAt: null,
    channelType,
    description: "",
    goalsEnabled: true,
    groupChat,
    id: row.channel_id,
    isMember: true,
    lastMessageAt:
      latestAt !== null ? new Date(latestAt * 1000).toISOString() : null,
    memberCount: Math.max(row.member_count ?? 0, others.length + 1),
    memberPubkeys: [],
    name: titledByPeople
      ? formatDmParticipantLabels(
          others.map((pubkey) => labelOf(pubkey) ?? truncateNpub(pubkey)),
        )
      : row.channel_name,
    participantPubkeys: [],
    participants: [],
    purpose: null,
    topic: null,
    ttlDeadline: null,
    ttlSeconds: null,
    visibility: "private",
  };
  standIns.add(channel);
  return channel;
}

/** What a row contributes to the opened channel, as a stable string. */
function rowKey(row: ChatListRow | null): string {
  if (!row) return "";
  return JSON.stringify([
    row.channel_id,
    row.channel_type,
    row.channel_name,
    row.group_chat === true,
    row.member_count ?? 0,
    row.dm_members,
  ]);
}

/**
 * Room `channelId` from the shown Chats rows, or null (no row, or
 * `enabled` false). `scope` is the account and server drawn under; its kept
 * profile names title a DM. Re-renders only when what the row says, or the
 * kept names, change.
 */
export function useChatRowChannel(
  channelId: string | null,
  enabled: boolean,
  scope: UserLabelScope | null,
): Channel | null {
  const queryClient = useQueryClient();
  const shown = shownIn(queryClient);
  const subscribe = React.useCallback(
    (listener: () => void) => {
      shown.listeners.add(listener);
      return () => {
        shown.listeners.delete(listener);
      };
    },
    [shown],
  );
  const key = React.useSyncExternalStore(subscribe, () =>
    enabled && channelId ? rowKey(chatRowForRoom(shown.rows, channelId)) : "",
  );
  const labelsVersion = React.useSyncExternalStore(
    subscribeUserLabels,
    getUserLabelsVersion,
  );
  const self = scope?.pubkey ?? null;
  const relayUrl = scope?.relayUrl ?? null;
  // biome-ignore lint/correctness/useExhaustiveDependencies: keyed by what the row says (rowKey) and the kept names' version
  return React.useMemo(() => {
    if (!key || !channelId) return null;
    const row = chatRowForRoom(shown.rows, channelId);
    if (!row) return null;
    const labelScope = self && relayUrl ? { pubkey: self, relayUrl } : null;
    return channelFromChatRow(row, self, (pubkey) =>
      resolveKnownUserLabel({
        profiles: readCachedUserLabels(labelScope, [pubkey])?.profiles,
        pubkey,
      }),
    );
  }, [key, self, relayUrl, labelsVersion]);
}
