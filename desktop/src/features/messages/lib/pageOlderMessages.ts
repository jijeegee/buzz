import type { QueryClient } from "@tanstack/react-query";

import {
  appendOlderChannelWindow,
  type ChannelWindowCursor,
  type ChannelWindowStore,
} from "@/features/messages/lib/channelWindowStore";
import { projectChannelWindowMessages } from "@/features/messages/lib/projectChannelWindow";
import { parseChannelWindowResponse } from "@/features/messages/lib/channelWindowResponse";
import { channelWindowKey } from "@/features/messages/lib/messageQueryKeys";
import { markRenderScopedReactionsLoaded } from "@/features/messages/lib/renderScopedReactions";
import { checkedWindow } from "@/features/messages/lib/windowCheck";
import { roomWindowIo } from "@/features/messages/lib/windowCheckIo";
import type { RelayEvent } from "@/shared/api/types";

const CHANNEL_WINDOW_PAGE_SIZE = 50;
export type PageOlderResult = { hasOlderMessages: boolean };
const inFlightPasses = new Map<string, Promise<PageOlderResult>>();

/**
 * Fetch exactly one server-defined older window (`pageSize` rows, at most the
 * relay's 200) and append it atomically.
 */
export function pageOlderMessagesUntilRowFloor(
  queryClient: QueryClient,
  channelId: string,
  shouldContinue: () => boolean,
  pageSize = CHANNEL_WINDOW_PAGE_SIZE,
): Promise<PageOlderResult> {
  const running = inFlightPasses.get(channelId);
  if (running) return running;
  const pass = runPage(
    queryClient,
    channelId,
    shouldContinue,
    pageSize,
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
  pageSize: number,
): Promise<PageOlderResult> {
  const store = queryClient.getQueryData<ChannelWindowStore>(
    channelWindowKey(channelId),
  );
  const tail = store?.pages[store.pages.length - 1];
  if (!store || !tail?.hasMore || !tail.nextCursor || !shouldContinue()) {
    return { hasOlderMessages: false };
  }

  const requestCursor = tail.nextCursor;
  // The stored window, when the store holds it all, is drawn at once; the
  // open's answer then takes its place (docs/message-sync.md#window-check).
  let painted = false;
  const apply = (events: RelayEvent[], replacePainted: boolean) => {
    const page = parseChannelWindowResponse(events, channelId, requestCursor);
    const retained = queryClient.getQueryData<ChannelWindowStore>(
      channelWindowKey(channelId),
    );
    if (!retained) return null;
    const last = retained.pages[retained.pages.length - 1];
    const base =
      replacePainted && last && cursorsMatch(last.startCursor, requestCursor)
        ? { ...retained, pages: retained.pages.slice(0, -1) }
        : retained;
    const next = appendOlderChannelWindow(base, page);
    markRenderScopedReactionsLoaded(
      channelId,
      page.rows.map((row) => row.event.id),
    );
    queryClient.setQueryData(channelWindowKey(channelId), next);
    projectChannelWindowMessages(queryClient, channelId);
    return page;
  };
  const paint = (events: RelayEvent[]) => {
    if (painted || !shouldContinue()) return;
    try {
      painted = apply(events, false) !== null;
    } catch {
      // A stored page that does not fit is left for the answer.
    }
  };
  const events = await checkedWindow(
    queryClient,
    { roomId: channelId, rootId: null, cursor: requestCursor, limit: pageSize },
    roomWindowIo(channelId, requestCursor, pageSize, {}, paint),
  );
  if (!shouldContinue() && !painted) return { hasOlderMessages: true };
  const page = apply(events, painted);
  if (!page) return { hasOlderMessages: true };
  return { hasOlderMessages: page.hasMore };
}

function cursorsMatch(
  left: ChannelWindowCursor | null,
  right: ChannelWindowCursor | null,
) {
  return (
    left === right ||
    (left !== null &&
      right !== null &&
      left.createdAt === right.createdAt &&
      left.eventId === right.eventId)
  );
}
