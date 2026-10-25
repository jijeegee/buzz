import type { QueryClient } from "@tanstack/react-query";

import {
  appendOlderChannelWindow,
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
export type PageOlderResult = { hasOlderMessages: boolean };
const inFlightPasses = new Map<string, Promise<PageOlderResult>>();

/** Fetch exactly one server-defined older window and append it atomically. */
export function pageOlderMessagesUntilRowFloor(
  queryClient: QueryClient,
  channelId: string,
  shouldContinue: () => boolean,
): Promise<PageOlderResult> {
  const running = inFlightPasses.get(channelId);
  if (running) return running;
  const pass = runPage(queryClient, channelId, shouldContinue).finally(() => {
    inFlightPasses.delete(channelId);
  });
  inFlightPasses.set(channelId, pass);
  return pass;
}

async function runPage(
  queryClient: QueryClient,
  channelId: string,
  shouldContinue: () => boolean,
): Promise<PageOlderResult> {
  const store = queryClient.getQueryData<ChannelWindowStore>(
    channelWindowKey(channelId),
  );
  const tail = store?.pages[store.pages.length - 1];
  if (!store || !tail?.hasMore || !tail.nextCursor || !shouldContinue()) {
    return { hasOlderMessages: false };
  }

  const requestCursor = tail.nextCursor;
  const page =
    (await readStoredOlderPage(queryClient, channelId, requestCursor)) ??
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
  return { hasOlderMessages: page.hasMore };
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
