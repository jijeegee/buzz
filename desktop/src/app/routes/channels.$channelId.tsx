import * as React from "react";
import { createFileRoute, useLocation } from "@tanstack/react-router";

import { LazyChannelRouteScreen } from "@/app/routes/lazyChannelRouteScreen";
import { nextWarmRooms, type WarmRooms } from "@/app/routes/warmRooms";
import { useDevTuning } from "@/shared/lib/devTuning";
import { selectSearchHighlightRouteState } from "@/app/routes/searchHighlightRouteState";

import {
  parseProfilePanelTab,
  parseProfilePanelView,
  type ProfilePanelTab,
  type ProfilePanelView,
} from "@/features/profile/ui/UserProfilePanelUtils";
import { HuddleStartingView } from "@/features/huddle/components/HuddleStartingView";
import { huddleWindowChannelId } from "@/features/huddle/lib/huddleWindow";
import { cn } from "@/shared/lib/cn";
import { ViewLoadingFallback } from "@/shared/ui/ViewLoadingFallback";
import {
  setInboxThreadViewMode,
  ThreadViewModeOverrideProvider,
  useInboxThreadViewMode,
} from "@/features/channels/lib/threadViewModePreference";
import { useInboxPanelOpen } from "@/features/home/lib/inboxPanelPreference";
import { InboxPanel } from "@/features/home/ui/InboxPanel";

type ChannelRouteSearch = {
  agentSession?: string;
  agentSessionReturnThread?: string;
  /**
   * When set, the composer on mount will auto-submit its loaded draft once,
   * then clear this param. Value is the draft key that was loaded so the
   * composer can verify it has the right draft before firing.
   */
  autoSend?: string;
  /** "1": jump to `messageId` without opening its reply panel. */
  jump?: string;
  messageId?: string;
  profile?: string;
  profileTab?: ProfilePanelTab;
  profileView?: ProfilePanelView;
  thread?: string;
  threadRootId?: string;
};

function nonEmptyString(value: unknown): string | undefined {
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

function validateChannelSearch(
  search: Record<string, unknown>,
): ChannelRouteSearch {
  return {
    agentSession: nonEmptyString(search.agentSession),
    agentSessionReturnThread: nonEmptyString(search.agentSessionReturnThread),
    autoSend: nonEmptyString(search.autoSend),
    jump: nonEmptyString(search.jump),
    messageId: nonEmptyString(search.messageId),
    profile: nonEmptyString(search.profile),
    profileTab: parseProfilePanelTab(search.profileTab) ?? undefined,
    profileView: parseProfilePanelView(search.profileView) ?? undefined,
    thread: nonEmptyString(search.thread),
    threadRootId: nonEmptyString(search.threadRootId),
  };
}

export const Route = createFileRoute("/channels/$channelId")({
  validateSearch: validateChannelSearch,
  component: ChannelRouteComponent,
});

const ChannelRouteScreen = LazyChannelRouteScreen;

function useWarmRooms(channelId: string, warmRooms: number) {
  const [state, setState] = React.useState<WarmRooms>({
    recency: [channelId],
    order: [channelId],
  });
  const next = React.useMemo(
    () => nextWarmRooms(state, channelId, warmRooms),
    [channelId, state, warmRooms],
  );
  React.useEffect(() => {
    setState((current) =>
      current.order.join("|") === next.order.join("|") &&
      current.recency.join("|") === next.recency.join("|")
        ? current
        : next,
    );
  }, [next]);
  return next.order;
}

function ChannelRouteComponent() {
  const { channelId } = Route.useParams();
  const search = Route.useSearch();
  const searchHighlight = useLocation({
    select: selectSearchHighlightRouteState,
  });
  const isHuddleTranscript = huddleWindowChannelId() !== null;
  const inboxPanelOpen = useInboxPanelOpen() && !isHuddleTranscript;
  // With the inbox pulled out, threads use the inbox's own remembered layout
  // (maximized over the collapsed channel unless switched). The toggle here
  // changes only that inbox layout; the channel default is untouched.
  const inboxThreadViewMode = useInboxThreadViewMode();
  const threadViewModeOverride = React.useMemo(
    () =>
      inboxPanelOpen
        ? { mode: inboxThreadViewMode, setMode: setInboxThreadViewMode }
        : null,
    [inboxPanelOpen, inboxThreadViewMode],
  );

  // Recently opened rooms stay rendered but hidden (React Activity: effects,
  // subscriptions and read marking pause while hidden), so switching back to
  // one shows it at once, scrolled where it was. The count is a dev knob.
  const { warmRooms, loaded: devTuningLoaded } = useDevTuning();
  const recentChannelIds = useWarmRooms(channelId, warmRooms);

  const openChannelScreen = (
    <ChannelRouteScreen
      autoSendDraftKey={search.autoSend ?? null}
      channelId={channelId}
      searchHighlight={searchHighlight}
      selectedPostId={null}
      targetMessageId={search.messageId ?? null}
      targetReplyId={null}
      targetThreadRootId={search.threadRootId ?? search.thread ?? null}
    />
  );

  const channelScreen = (
    <React.Suspense
      fallback={
        isHuddleTranscript ? (
          <HuddleStartingView />
        ) : (
          <ViewLoadingFallback includeHeader kind="channel" />
        )
      }
    >
      <ThreadViewModeOverrideProvider value={threadViewModeOverride}>
        {!devTuningLoaded ? (
          // Read at launch in a few milliseconds; choosing the layout before
          // it arrives would remount the open room once it does.
          <ViewLoadingFallback includeHeader kind="channel" />
        ) : warmRooms <= 1 ? (
          // One room: the single screen follows the route, as it always has.
          openChannelScreen
        ) : (
          recentChannelIds.map((id) =>
            id === channelId ? (
              <React.Activity key={id} mode="visible">
                {openChannelScreen}
              </React.Activity>
            ) : (
              <React.Activity key={id} mode="hidden">
                <ChannelRouteScreen
                  autoSendDraftKey={null}
                  channelId={id}
                  searchHighlight={undefined}
                  selectedPostId={null}
                  targetMessageId={null}
                  targetReplyId={null}
                  targetThreadRootId={null}
                />
              </React.Activity>
            ),
          )
        )}
      </ThreadViewModeOverrideProvider>
    </React.Suspense>
  );

  // One stable tree whether or not the panel is out, so toggling the inbox
  // never remounts the chat screen (scroll, drafts and thread stay put).
  return (
    <div
      className="flex min-h-0 min-w-0 flex-1 overflow-hidden"
      data-testid="channel-route-layout"
    >
      {inboxPanelOpen ? <InboxPanel /> : null}
      <div
        className={cn(
          "flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden",
          inboxPanelOpen && "border-l border-border/35",
        )}
      >
        {channelScreen}
      </div>
    </div>
  );
}
