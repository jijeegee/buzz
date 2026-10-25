import type { QueryClient } from "@tanstack/react-query";

import type { ThreadWindowPage } from "@/shared/api/threadWindow";
import {
  type MessageStoreThreadHeadPage,
  readMessageStoreThreadHead,
} from "@/shared/api/tauriMessageStore";
import type { Channel, ThreadCursor } from "@/shared/api/types";
import { storeScopeFor } from "./storeChannelHeads";

/** The room's last activity time from the channel list (null when unknown). */
export function channelLastMessageAt(
  queryClient: QueryClient,
  channelId: string,
): string | null {
  return (
    queryClient
      .getQueryData<Channel[]>(["channels"])
      ?.find((channel) => channel.id === channelId)?.lastMessageAt ?? null
  );
}

/**
 * A stored newest thread page may paint with no server request only when it
 * did not stop at a coverage gap, and either the room's live segment
 * continues it or its coverage reaches the room's last activity (any event,
 * replies included, so no reply can be newer). An unknown time proves nothing.
 */
export function isProvenThreadHead(
  page: Pick<
    MessageStoreThreadHeadPage,
    "gap" | "liveCovered" | "coveredThrough"
  >,
  lastMessageAt: string | null,
): boolean {
  if (page.gap) return false;
  if (page.liveCovered) return true;
  const ms = lastMessageAt === null ? Number.NaN : Date.parse(lastMessageAt);
  return (
    Number.isFinite(ms) &&
    page.coveredThrough !== null &&
    page.coveredThrough >= Math.floor(ms / 1000)
  );
}

/** Map a store thread page onto the server's thread window page shape. */
export function storeThreadPage(
  page: MessageStoreThreadHeadPage,
  channelId: string,
  rootId: string,
): ThreadWindowPage {
  return {
    channelId,
    rootId,
    events: [...page.rows, ...page.aux],
    // On a gap the next older load starts at the last stored reply; its
    // store read stops there at once and asks the server.
    hasMore: page.hasMore,
    nextCursor: page.nextCursor,
  };
}

async function readThreadHead(
  queryClient: QueryClient,
  rootId: string,
  before: ThreadCursor | null,
  limitRows: number,
): Promise<MessageStoreThreadHeadPage | null> {
  const scope = storeScopeFor(queryClient);
  if (!scope) return null;
  try {
    return await readMessageStoreThreadHead(scope, rootId, before, limitRows);
  } catch (error) {
    console.warn("Failed to read stored thread replies", rootId, error);
    return null;
  }
}

/**
 * A thread's newest page from the store, only when the store proves it is
 * the thread's current newest page (see `isProvenThreadHead`).
 */
export async function readProvenStoreThread(
  queryClient: QueryClient,
  channelId: string,
  rootId: string,
  limitRows: number,
  lastMessageAt: string | null = channelLastMessageAt(queryClient, channelId),
): Promise<ThreadWindowPage | null> {
  const page = await readThreadHead(queryClient, rootId, null, limitRows);
  if (!page || !isProvenThreadHead(page, lastMessageAt)) return null;
  return storeThreadPage(page, channelId, rootId);
}

/**
 * Replies the store holds right below `cursor`, inside covered history.
 * Null when it holds none there: the server answers.
 */
export async function readStoredOlderThreadPage(
  queryClient: QueryClient,
  channelId: string,
  rootId: string,
  cursor: ThreadCursor,
  limitRows: number,
): Promise<ThreadWindowPage | null> {
  const page = await readThreadHead(queryClient, rootId, cursor, limitRows);
  if (!page || page.rows.length === 0) return null;
  return storeThreadPage(page, channelId, rootId);
}
