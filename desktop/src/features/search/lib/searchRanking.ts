import { scoreChannelName } from "@/features/channels/lib/channelSearchScore";
import type { ChatListRow } from "@/features/chats/chatListState";

/**
 * `scoreChannelName` bands: exact 0, prefix 1, word 2-3, middle 4. Thread
 * names stop at a plain substring match; the fuzzy bands are for channels.
 */
const THREAD_NAME_MAX_SCORE = 4;

/** A named thread from the Chats list, as a search result. */
export type SearchThread = {
  /** Seconds; the thread's latest activity. */
  at: number;
  channelId: string;
  /** The relay's channel name; the app's own label wins when present. */
  channelName: string;
  name: string;
  rootId: string;
};

/** One group, DM, or thread match, ranked by {@link compareGroupMatches}. */
export type GroupMatch = {
  /** Milliseconds; 0 when unknown. */
  activity: number;
  /** Unjoined public groups go last in their section. */
  joined: boolean;
  label: string;
  /** Lower is better. */
  score: number;
};

/**
 * Joined before unjoined, then match quality (exact > prefix > word start >
 * middle), then the most recent activity, then the name.
 */
export function compareGroupMatches(left: GroupMatch, right: GroupMatch) {
  return (
    Number(!left.joined) - Number(!right.joined) ||
    left.score - right.score ||
    right.activity - left.activity ||
    left.label.localeCompare(right.label)
  );
}

/**
 * Named threads in the Chats list whose name contains `lowerQuery`
 * ("검색" finds "채널 검색 및 초대"), with their match score. Threads in a
 * channel `isVisibleChannel` rejects are skipped.
 */
export function findThreadNameMatches(
  rows: readonly ChatListRow[],
  lowerQuery: string,
  isVisibleChannel: (channelId: string) => boolean,
): Array<{ score: number; thread: SearchThread }> {
  if (!lowerQuery) return [];
  const seen = new Set<string>();
  const matches: Array<{ score: number; thread: SearchThread }> = [];
  for (const row of rows) {
    const rootId = row.root_id;
    const name = row.thread_name?.trim();
    if (row.type !== "thread" || !rootId || !name || seen.has(rootId)) {
      continue;
    }
    if (!isVisibleChannel(row.channel_id)) continue;
    const score = scoreChannelName(name, lowerQuery);
    if (score === null || score > THREAD_NAME_MAX_SCORE) continue;
    seen.add(rootId);
    matches.push({
      score,
      thread: {
        at: row.at,
        channelId: row.channel_id,
        channelName: row.channel_name,
        name,
        rootId,
      },
    });
  }
  return matches;
}

/** Message results newest first (server order breaks ties). */
export function sortNewestFirst<T extends { createdAt: number }>(
  hits: readonly T[],
): T[] {
  return hits
    .map((hit, index) => ({ hit, index }))
    .sort(
      (left, right) =>
        right.hit.createdAt - left.hit.createdAt || left.index - right.index,
    )
    .map(({ hit }) => hit);
}
