import type {
  ActiveChannelTurnSummary,
  ActiveWorkingTurn,
} from "@/features/agents/activeAgentTurnsStore";
import type { ChatListRow } from "./chatListState";

export type ChatWorking = {
  /** Work shown on room rows (channel body, DM, or thread not listed). */
  rooms: ReadonlyMap<string, ActiveChannelTurnSummary>;
  /** Work shown on listed thread rows, by thread root id. */
  threads: ReadonlyMap<string, ActiveChannelTurnSummary>;
};

function subset(
  summary: ActiveChannelTurnSummary,
  pubkeys: readonly string[],
  anchorAt: number,
): ActiveChannelTurnSummary {
  // Names line up with pubkeys only when every agent has a name.
  const names =
    summary.agentNames?.length === summary.agentPubkeys.length
      ? summary.agentNames
      : null;
  const nameOf = (pubkey: string) =>
    names ? names[summary.agentPubkeys.indexOf(pubkey)] : undefined;
  return {
    agentCount: pubkeys.length,
    agentNames: names ? pubkeys.map((pubkey) => nameOf(pubkey) ?? "") : [],
    agentPubkeys: [...pubkeys],
    anchorAt,
    channelId: summary.channelId,
  };
}

function summarize(
  summary: ActiveChannelTurnSummary,
  turns: readonly ActiveWorkingTurn[],
  extraPubkeys: readonly string[] = [],
): ActiveChannelTurnSummary {
  const pubkeys = [
    ...new Set([...turns.map((turn) => turn.agentPubkey), ...extraPubkeys]),
  ];
  const anchorAt =
    turns.length > 0
      ? Math.min(...turns.map((turn) => turn.anchorAt))
      : summary.anchorAt;
  return subset(summary, pubkeys, anchorAt);
}

/**
 * Puts each agent's work on the row it belongs to: a turn in a listed
 * thread goes to that thread's row, everything else (channel body, DM, a
 * thread not in the list, typing-only work) stays on the room row, so the
 * same work never shows twice.
 */
export function splitChatWorking(
  byChannel: ReadonlyMap<string, ActiveChannelTurnSummary> | undefined,
  rows: readonly ChatListRow[],
): ChatWorking {
  const rooms = new Map<string, ActiveChannelTurnSummary>();
  const threads = new Map<string, ActiveChannelTurnSummary>();
  if (!byChannel) return { rooms, threads };
  const listedThreads = new Set(
    rows
      .filter((row) => row.type !== "room")
      .map((row) => `${row.channel_id}:${row.root_id ?? row.key}`),
  );
  for (const [channelId, summary] of byChannel) {
    const turns = summary.turns ?? [];
    if (turns.length === 0) {
      rooms.set(channelId, summary);
      continue;
    }
    const roomTurns: ActiveWorkingTurn[] = [];
    const threadTurns = new Map<string, ActiveWorkingTurn[]>();
    for (const turn of turns) {
      const root = turn.threadRootEventId;
      if (root && listedThreads.has(`${channelId}:${root}`)) {
        threadTurns.set(root, [...(threadTurns.get(root) ?? []), turn]);
      } else {
        roomTurns.push(turn);
      }
    }
    // Agents seen only by typing have no turn; they stay on the room row.
    const withTurns = new Set(turns.map((turn) => turn.agentPubkey));
    const typingOnly = summary.agentPubkeys.filter(
      (pubkey) => !withTurns.has(pubkey),
    );
    if (roomTurns.length > 0 || typingOnly.length > 0) {
      rooms.set(channelId, summarize(summary, roomTurns, typingOnly));
    }
    for (const [root, list] of threadTurns) {
      threads.set(root, summarize(summary, list));
    }
  }
  return { rooms, threads };
}
