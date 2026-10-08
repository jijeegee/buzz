import * as React from "react";

import { useAppShell } from "@/app/AppShellContext";
import { markHiddenDmFeedItems } from "@/features/channels/dmResurface";
import { useHiddenDmIds } from "@/features/channels/useHiddenDmIds";
import { useChannelsQuery } from "@/features/channels/hooks";
import {
  useConversationChannelActivity,
  useHomeFeedQuery,
  useInboxDeletedEventIds,
} from "@/features/home/hooks";
import { useInboxFilter } from "@/features/home/lib/inboxFilterPreference";
import {
  collectInboxReferencedEventIds,
  withoutDeletedFeedItems,
} from "@/features/home/lib/inboxDeletions";
import { HomeView } from "@/features/home/ui/HomeView";
import type { HomeFeedResponse } from "@/shared/api/types";
import {
  isRelayUnreachableError,
  RELAY_UNREACHABLE_MESSAGE,
} from "@/shared/lib/relayError";

type HomeScreenProps = {
  availableChannelIds: ReadonlySet<string>;
  currentPubkey?: string;
  onOpenContext: (
    channelId: string,
    messageId: string | null,
    threadRootId?: string | null,
  ) => void;
  /** `panel` renders only the list, beside a channel's chat screen. */
  variant?: "page" | "panel";
};

export function HomeScreen({
  availableChannelIds,
  currentPubkey,
  onOpenContext,
  variant = "page",
}: HomeScreenProps) {
  const homeFeedQuery = useHomeFeedQuery();
  const { threadActivityFeedItems } = useAppShell();
  const hiddenDmIds = useHiddenDmIds(currentPubkey);

  // Channels + Threads is a chat list: it also loads the rooms' recent
  // traffic, including the user's own messages, so every room appears and
  // sorts by its real latest activity.
  const isConversationView = useInboxFilter() === "conversations";
  const conversationActivity = useConversationChannelActivity(
    useChannelsQuery().data,
    isConversationView,
  );
  const augmentedFeed = React.useMemo((): HomeFeedResponse | undefined => {
    if (!homeFeedQuery.data) return undefined;
    const extraActivity = [
      ...threadActivityFeedItems,
      ...(isConversationView ? (conversationActivity ?? []) : []),
    ];
    // Feed items win on duplicate ids: their category is more specific.
    const feedIds = new Set(
      [
        ...homeFeedQuery.data.feed.mentions,
        ...homeFeedQuery.data.feed.needsAction,
        ...homeFeedQuery.data.feed.activity,
        ...homeFeedQuery.data.feed.agentActivity,
      ].map((item) => item.id),
    );
    const seen = new Set<string>();
    const newActivity = extraActivity.filter((item) => {
      if (feedIds.has(item.id) || seen.has(item.id)) return false;
      seen.add(item.id);
      return true;
    });
    const withThreadActivity =
      newActivity.length === 0
        ? homeFeedQuery.data
        : {
            ...homeFeedQuery.data,
            feed: {
              ...homeFeedQuery.data.feed,
              activity: [...homeFeedQuery.data.feed.activity, ...newActivity],
            },
          };
    return markHiddenDmFeedItems(withThreadActivity, hiddenDmIds);
  }, [
    conversationActivity,
    hiddenDmIds,
    homeFeedQuery.data,
    isConversationView,
    threadActivityFeedItems,
  ]);
  const referencedEventIds = React.useMemo(
    () =>
      augmentedFeed ? collectInboxReferencedEventIds(augmentedFeed.feed) : [],
    [augmentedFeed],
  );
  const deletedEventIds = useInboxDeletedEventIds(referencedEventIds).data;
  // A deleted message, or a deleted thread root, takes its inbox rows with it.
  const visibleFeed = React.useMemo(
    () =>
      augmentedFeed && deletedEventIds
        ? withoutDeletedFeedItems(augmentedFeed, deletedEventIds)
        : augmentedFeed,
    [augmentedFeed, deletedEventIds],
  );

  return (
    <div
      className={
        variant === "panel"
          ? "flex min-h-0 shrink-0 flex-col overflow-hidden"
          : "flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden"
      }
    >
      <HomeView
        availableChannelIds={availableChannelIds}
        currentPubkey={currentPubkey}
        errorMessage={
          homeFeedQuery.error !== null && homeFeedQuery.error !== undefined
            ? isRelayUnreachableError(homeFeedQuery.error)
              ? RELAY_UNREACHABLE_MESSAGE
              : homeFeedQuery.error instanceof Error
                ? homeFeedQuery.error.message
                : undefined
            : undefined
        }
        feed={visibleFeed}
        isLoading={homeFeedQuery.isLoading}
        onOpenContext={onOpenContext}
        onRefresh={() => {
          void homeFeedQuery.refetch();
        }}
        variant={variant}
      />
    </div>
  );
}
