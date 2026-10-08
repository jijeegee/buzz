import * as React from "react";
import { createFileRoute, useLocation } from "@tanstack/react-router";

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
  type ThreadViewMode,
  ThreadViewModeOverrideProvider,
} from "@/features/channels/lib/threadViewModePreference";
import { LandAtLatestProvider } from "@/features/channels/lib/landAtLatest";
import { RootTargetOpensThreadProvider } from "@/features/channels/ui/useChannelRouteTarget";
import {
  useInboxPanelOpen,
  useInboxRoomLanding,
} from "@/features/home/lib/inboxPanelPreference";
import { InboxPanel } from "@/features/home/ui/InboxPanel";

type ChannelRouteSearch = {
  agentSession?: string;
  /**
   * When set, the composer on mount will auto-submit its loaded draft once,
   * then clear this param. Value is the draft key that was loaded so the
   * composer can verify it has the right draft before firing.
   */
  autoSend?: string;
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
    autoSend: nonEmptyString(search.autoSend),
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

const ChannelRouteScreen = React.lazy(async () => {
  const module = await import("./ChannelRouteScreen");
  return { default: module.ChannelRouteScreen };
});

function ChannelRouteComponent() {
  const { channelId } = Route.useParams();
  const search = Route.useSearch();
  const searchHighlight = useLocation({
    select: selectSearchHighlightRouteState,
  });
  const isHuddleTranscript = huddleWindowChannelId() !== null;
  const inboxPanelOpen = useInboxPanelOpen() && !isHuddleTranscript;
  // An inbox entry lands exactly where its row points: a fresh chat mount per
  // entry (so clicking the same row again lands again), and a room row scrolls
  // to its latest message without opening a thread.
  const landing = useInboxRoomLanding();
  const landingHere = landing?.channelId === channelId ? landing : null;
  const landAtLatest =
    landingHere !== null &&
    landingHere.messageId === null &&
    search.messageId === undefined;
  const rootTargetOpensThread =
    landingHere === null ||
    search.messageId !== landingHere.messageId ||
    landingHere.opensThread;
  // With the inbox pulled out, threads open maximized over the collapsed
  // channel. The layout toggle only switches this view while the panel is open;
  // the saved channel default is untouched.
  const [inboxThreadViewMode, setInboxThreadViewMode] =
    React.useState<ThreadViewMode>("focus");
  React.useEffect(() => {
    if (inboxPanelOpen) setInboxThreadViewMode("focus");
  }, [inboxPanelOpen]);
  const threadViewModeOverride = React.useMemo(
    () =>
      inboxPanelOpen
        ? { mode: inboxThreadViewMode, setMode: setInboxThreadViewMode }
        : null,
    [inboxPanelOpen, inboxThreadViewMode],
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
        <RootTargetOpensThreadProvider value={rootTargetOpensThread}>
          <LandAtLatestProvider value={landAtLatest}>
            <ChannelRouteScreen
              key={landingHere?.nonce ?? 0}
              autoSendDraftKey={search.autoSend ?? null}
              channelId={channelId}
              searchHighlight={searchHighlight}
              selectedPostId={null}
              targetMessageId={search.messageId ?? null}
              targetReplyId={null}
              targetThreadRootId={search.threadRootId ?? search.thread ?? null}
            />
          </LandAtLatestProvider>
        </RootTargetOpensThreadProvider>
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
