import * as React from "react";
import { useHuddle } from "@/features/huddle/HuddleContext";
import { huddleWindowChannelId } from "@/features/huddle/lib/huddleWindow";
import { mergeMessages } from "@/features/messages/hooks";
import {
  channelWindowThreadSummaries,
  type ChannelWindowStore,
} from "@/features/messages/lib/channelWindowStore";
import { isSearchHitPlaceholder } from "@/features/messages/lib/searchHitPlaceholder";
import { useThreadRepliesForRoots } from "@/features/messages/useThreadReplies";
import type { Channel, RelayEvent } from "@/shared/api/types";

export function useIsHuddleTranscript(activeChannelId: string | null) {
  const { activeEphemeralChannelId } = useHuddle();
  return (
    huddleWindowChannelId() !== null ||
    (activeChannelId !== null && activeChannelId === activeEphemeralChannelId)
  );
}

type HuddleChannelMessagesOptions = {
  activeChannel: Channel | null;
  isHuddleTranscript: boolean;
  messages: RelayEvent[];
  targetMessageEvents: RelayEvent[];
  windowStore?: ChannelWindowStore;
};

export function useHuddleChannelMessages({
  activeChannel,
  isHuddleTranscript,
  messages,
  targetMessageEvents,
  windowStore,
}: HuddleChannelMessagesOptions) {
  const resolvedChannelMessages = React.useMemo(() => {
    // A search-hit placeholder must not replace the feed's own copy: it lacks
    // thread tags, so it would render a loaded thread reply as a channel row.
    const loadedIds = new Set(messages.map((message) => message.id));
    const extraEvents = targetMessageEvents.filter(
      (event) =>
        !isSearchHitPlaceholder(event.tags) || !loadedIds.has(event.id),
    );
    if (!activeChannel || extraEvents.length === 0) return messages;
    return extraEvents.reduce(mergeMessages, messages);
  }, [activeChannel, messages, targetMessageEvents]);

  const threadSummaries = React.useMemo(
    () => (windowStore ? channelWindowThreadSummaries(windowStore) : new Map()),
    [windowStore],
  );
  const huddleThreadRootIds = React.useMemo(
    () =>
      isHuddleTranscript
        ? [...threadSummaries.entries()]
            .filter(([, summary]) => summary.descendantCount > 0)
            .map(([rootId]) => rootId)
        : [],
    [isHuddleTranscript, threadSummaries],
  );
  const huddleThreadReplies = useThreadRepliesForRoots(
    activeChannel,
    huddleThreadRootIds,
  );
  const resolvedMessages = React.useMemo(
    () =>
      isHuddleTranscript
        ? huddleThreadReplies.events.reduce(
            mergeMessages,
            resolvedChannelMessages,
          )
        : resolvedChannelMessages,
    [huddleThreadReplies.events, isHuddleTranscript, resolvedChannelMessages],
  );

  return {
    resolvedMessages,
    threadSummaries,
    // A summarized reply subtree failing must not leave the transcript reading
    // as complete: surface the aggregate failure so the consumer can show a
    // non-destructive retry alert alongside the rows that did load.
    threadRepliesError: isHuddleTranscript && huddleThreadReplies.isError,
    onRetryThreadReplies: huddleThreadReplies.refetch,
  };
}
