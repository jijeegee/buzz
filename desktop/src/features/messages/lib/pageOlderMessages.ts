import type { QueryClient } from "@tanstack/react-query";

import {
  appendOlderChannelWindow,
  type ChannelWindowPage,
  type ChannelWindowStore,
} from "@/features/messages/lib/channelWindowStore";
import { projectChannelWindowMessages } from "@/features/messages/lib/projectChannelWindow";
import { parseChannelWindowResponse } from "@/features/messages/lib/channelWindowResponse";
import { channelWindowKey } from "@/features/messages/lib/messageQueryKeys";
import {
  storeScopeFor,
  storeWindowPage,
} from "@/features/messages/lib/storeChannelHeads";
import type { ChannelWindowCursor } from "@/features/messages/lib/channelWindowStore";
import { getChannelWindowEvents } from "@/shared/api/channelWindow";
import { readMessageStoreWindow } from "@/shared/api/tauriMessageStore";

const CHANNEL_WINDOW_PAGE_SIZE = 50;
export type PageOlderResult = {
  hasOlderMessages: boolean;
  /** Where the appended page came from; absent when none was appended. */
  source?: "store" | "server";
};
const inFlightPasses = new Map<string, Promise<PageOlderResult>>();

/** Fetch exactly one server-defined older window and append it atomically. */
export function pageOlderMessagesUntilRowFloor(
  queryClient: QueryClient,
  channelId: string,
  shouldContinue: () => boolean,
  options: {
    /** Append only a page the local store holds; never ask the server. */
    storeOnly?: boolean;
  } = {},
): Promise<PageOlderResult> {
  const running = inFlightPasses.get(channelId);
  if (running) return running;
  const pass = runPage(
    queryClient,
    channelId,
    shouldContinue,
    options.storeOnly ?? false,
  ).finally(() => {
    inFlightPasses.delete(channelId);
  });
  inFlightPasses.set(channelId, pass);
  return pass;
}

async function runPage(
  queryClient: QueryClient,
  channelId: string,
  shouldContinue: () => boolean,
  storeOnly: boolean,
): Promise<PageOlderResult> {
  const store = queryClient.getQueryData<ChannelWindowStore>(
    channelWindowKey(channelId),
  );
  const tail = store?.pages[store.pages.length - 1];
  if (!store || !tail?.hasMore || !tail.nextCursor || !shouldContinue()) {
    return { hasOlderMessages: false };
  }

  const requestCursor = tail.nextCursor;
  const stored = await readStoredOlderPage(
    queryClient,
    channelId,
    requestCursor,
  );
  if (!stored && storeOnly) return { hasOlderMessages: true };
  const page =
    stored ??
    parseChannelWindowResponse(
      await getChannelWindowEvents(
        channelId,
        requestCursor,
        CHANNEL_WINDOW_PAGE_SIZE,
      ),
      channelId,
      requestCursor,
    );
  if (!shouldContinue()) return { hasOlderMessages: true };
  const retained = queryClient.getQueryData<ChannelWindowStore>(
    channelWindowKey(channelId),
  );
  if (!retained) return { hasOlderMessages: true };
  const next = appendOlderChannelWindow(retained, page);
  queryClient.setQueryData(channelWindowKey(channelId), next);
  projectChannelWindowMessages(queryClient, channelId);
  return {
    hasOlderMessages: page.hasMore,
    source: stored ? "store" : "server",
  };
}

/**
 * The stored rows just below `cursor`, when the store holds any inside the
 * covered run that contains it. Null asks the server: the cursor is outside
 * local coverage, or the run ends right below it.
 */
async function readStoredOlderPage(
  queryClient: QueryClient,
  channelId: string,
  cursor: ChannelWindowCursor,
) {
  const scope = storeScopeFor(queryClient);
  if (!scope) return null;
  try {
    const page = await readMessageStoreWindow(
      scope,
      channelId,
      cursor,
      CHANNEL_WINDOW_PAGE_SIZE,
    );
    return page.rows.length > 0 ? storeWindowPage(page, cursor) : null;
  } catch (error) {
    console.warn("Failed to read stored older rows", channelId, error);
    return null;
  }
}

/** Stored pages a head refetch may keep below its page (2000 rows). */
const MAX_KEPT_STORED_PAGES = 40;

/**
 * Stored pages continuing a fetched head's cursor chain down to the oldest
 * row the window holds now. A head refetch replaces the window at page zero;
 * these keep the older rows the reader already has (an open at the first
 * unread, scrolled history) so the list does not lose them and jump. Empty
 * when the window holds only its head or the store cannot continue the chain.
 */
export async function storedPagesBelowHead(
  queryClient: QueryClient,
  channelId: string,
  events: Awaited<ReturnType<typeof getChannelWindowEvents>>,
): Promise<ChannelWindowPage[]> {
  const current = queryClient.getQueryData<ChannelWindowStore>(
    channelWindowKey(channelId),
  );
  if (!current || current.pages.length < 2) return [];
  const floor = current.pages.at(-1)?.rows.at(-1)?.event;
  if (!floor) return [];
  const below: ChannelWindowPage[] = [];
  let tail = parseChannelWindowResponse(events, channelId, null);
  while (
    below.length < MAX_KEPT_STORED_PAGES &&
    tail.hasMore &&
    tail.nextCursor &&
    (tail.nextCursor.createdAt > floor.created_at ||
      (tail.nextCursor.createdAt === floor.created_at &&
        tail.nextCursor.eventId < floor.id))
  ) {
    const stored = await readStoredOlderPage(
      queryClient,
      channelId,
      tail.nextCursor,
    );
    if (!stored) break;
    below.push(stored);
    tail = stored;
  }
  return below;
}

/** `head` with the stored pages below it, or `head` alone if they do not chain. */
export function withStoredBelow(
  head: ChannelWindowStore,
  below: ChannelWindowPage[],
): ChannelWindowStore {
  try {
    return below.reduce(appendOlderChannelWindow, head);
  } catch {
    return head;
  }
}
