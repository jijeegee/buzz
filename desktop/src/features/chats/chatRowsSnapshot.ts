/**
 * The Chats rows kept on the device (docs/message-sync.md#launch-snapshots):
 * the server list as last shown, drawn at launch before the relay answers
 * and replaced by its first page. Kept per account and server in the device
 * message store; never used to decide anything but what is drawn.
 */
import {
  type QueryClient,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import * as React from "react";

import {
  type ChatListRow,
  chatRowContextId,
} from "@/features/chats/chatListState";
import { publishShownChatRows } from "@/features/chats/chatRowChannel";
import {
  createSnapshotWriter,
  readDeviceSnapshot,
  type SnapshotScope,
} from "@/shared/api/deviceSnapshot";

/** Rows kept: more than a window's worth, far below the loaded maximum. */
export const CHAT_ROWS_SNAPSHOT_ROWS = 200;
const SAVE_DELAY_MS = 2_000;

type ChatRowsSnapshot = { version: 1; rows: ChatListRow[] };

export function chatRowsSnapshotKey(relayUrl: string, pubkey: string) {
  return ["device-snapshot", "chat_rows", relayUrl, pubkey.toLowerCase()];
}

function isRow(value: unknown): value is ChatListRow {
  if (typeof value !== "object" || value === null) return false;
  const row = value as Record<string, unknown>;
  return (
    (row.type === "room" || row.type === "thread") &&
    typeof row.key === "string" &&
    typeof row.channel_id === "string" &&
    typeof row.at === "number" &&
    typeof row.at_us === "number" &&
    Array.isArray(row.dm_members)
  );
}

/** The rows of a stored blob; null for anything else. */
export function parseChatRowsSnapshot(
  blob: string | null,
): ChatListRow[] | null {
  if (!blob) return null;
  try {
    const parsed = JSON.parse(blob) as Partial<ChatRowsSnapshot> | null;
    if (parsed?.version !== 1 || !Array.isArray(parsed.rows)) return null;
    return parsed.rows.filter(isRow);
  } catch {
    return null;
  }
}

export async function readChatRowsSnapshot(
  scope: SnapshotScope,
): Promise<ChatListRow[] | null> {
  return parseChatRowsSnapshot(await readDeviceSnapshot("chat_rows", scope));
}

const writer = createSnapshotWriter("chat_rows", SAVE_DELAY_MS);

/** Keeps the first rows of the shown server list for the next launch. */
export function saveChatRowsSnapshot(
  scope: SnapshotScope,
  rows: readonly ChatListRow[],
) {
  writer.save(
    scope,
    JSON.stringify({
      version: 1,
      rows: rows.slice(0, CHAT_ROWS_SNAPSHOT_ROWS),
    } satisfies ChatRowsSnapshot),
  );
}

/** Drops a save still waiting (the account or community changed). */
export function cancelChatRowsSnapshot(scope?: SnapshotScope) {
  writer.cancel(scope);
}

/** Writes a waiting save for `scope` now. */
export function flushChatRowsSnapshot(scope: SnapshotScope) {
  writer.flush(scope);
}

/** Test seam. */
export function resetChatRowsSnapshotWriter() {
  writer.reset();
}

function snapshotQuery(scope: SnapshotScope) {
  return {
    queryKey: chatRowsSnapshotKey(scope.relayUrl, scope.pubkey),
    queryFn: () => readChatRowsSnapshot(scope),
    staleTime: Number.POSITIVE_INFINITY,
    gcTime: Number.POSITIVE_INFINITY,
    retry: false,
  } as const;
}

/** Starts reading the kept rows when the community's cache is made. */
export function prefetchChatRowsSnapshot(
  queryClient: QueryClient,
  scope: SnapshotScope,
) {
  void queryClient.prefetchQuery(snapshotQuery(scope));
}

/** The kept rows of this account and server, once read; else undefined. */
export function useChatRowsSnapshot(
  pubkey: string | undefined,
  relayUrl: string | undefined,
): readonly ChatListRow[] | undefined {
  const scope = pubkey && relayUrl ? { pubkey, relayUrl } : null;
  const query = useQuery({
    ...snapshotQuery(scope ?? { pubkey: "", relayUrl: "" }),
    enabled: scope !== null,
  });
  return scope ? (query.data ?? undefined) : undefined;
}

/**
 * The Chats rows to draw: the server's once it has answered this run; until
 * then (capability not yet probed, or on with the first page still out) the
 * kept rows. A relay that turned chat state off draws none.
 */
export function chatRowsToShow(
  capability: "unknown" | "on" | "off",
  serverRows: readonly ChatListRow[] | undefined,
  kept: readonly ChatListRow[] | undefined,
): readonly ChatListRow[] | undefined {
  if (capability === "off") return undefined;
  if (capability === "on" && serverRows) return serverRows;
  return kept && kept.length > 0 ? kept : undefined;
}

const keptLists = new WeakSet<readonly ChatListRow[]>();

/**
 * Whether `rows` are kept rows drawn before the relay answered: their row
 * actions (pin, unfollow, mark unread) wait for the relay's chat state.
 */
export function isKeptChatRows(rows: readonly ChatListRow[] | undefined) {
  return rows ? keptLists.has(rows) : false;
}

/**
 * Kept rows with this device's read positions over their kept counts and
 * marks: a room or thread read here since then (opening it) shows no badge.
 */
export function keptRowsWithLocalReads(
  rows: readonly ChatListRow[],
  getReadAt: (contextId: string) => number | null,
): readonly ChatListRow[] {
  const next = rows.map((row) => {
    const readAt = getReadAt(chatRowContextId(row));
    if (readAt === null) return row;
    if (row.latest && readAt < row.latest.created_at) return row;
    return {
      ...row,
      unread: 0,
      unread_overflow: false,
      unread_mentions: 0,
      unread_mentions_overflow: false,
      unread_mark: false,
    };
  });
  keptLists.add(next);
  return next;
}

/**
 * The Chats rows to draw for this account and server (see `chatRowsToShow`),
 * keeping the server's rows for the next launch as they change.
 */
export function useShownChatRows(
  capability: "unknown" | "on" | "off",
  serverRows: readonly ChatListRow[] | undefined,
  pubkey: string | undefined,
  relayUrl: string | undefined,
): readonly ChatListRow[] | undefined {
  const kept = useChatRowsSnapshot(pubkey, relayUrl);
  React.useEffect(() => {
    if (capability !== "on" || !serverRows || !pubkey || !relayUrl) return;
    saveChatRowsSnapshot({ pubkey, relayUrl }, serverRows);
  }, [capability, serverRows, pubkey, relayUrl]);
  React.useEffect(() => {
    if (!pubkey || !relayUrl) return;
    const scope = { pubkey, relayUrl };
    // A save still waiting belongs to this account only.
    return () => cancelChatRowsSnapshot(scope);
  }, [pubkey, relayUrl]);
  const shown = chatRowsToShow(capability, serverRows, kept);
  const queryClient = useQueryClient();
  // Rooms not in the channel list yet open from these rows (chatRowChannel).
  React.useEffect(() => {
    publishShownChatRows(queryClient, shown);
  }, [queryClient, shown]);
  React.useEffect(
    () => () => publishShownChatRows(queryClient, undefined),
    [queryClient],
  );
  return shown;
}
