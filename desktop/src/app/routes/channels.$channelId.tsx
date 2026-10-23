import * as React from "react";
import { createFileRoute, useLocation } from "@tanstack/react-router";

import { LazyChannelRouteScreen } from "@/app/routes/lazyChannelRouteScreen";
import { selectSearchHighlightRouteState } from "@/app/routes/searchHighlightRouteState";

import {
  parseProfilePanelTab,
  parseProfilePanelView,
  type ProfilePanelTab,
  type ProfilePanelView,
} from "@/features/profile/ui/UserProfilePanelUtils";
import { HuddleStartingView } from "@/features/huddle/components/HuddleStartingView";
import { huddleWindowChannelId } from "@/features/huddle/lib/huddleWindow";
import { ViewLoadingFallback } from "@/shared/ui/ViewLoadingFallback";
import {
  setInboxThreadViewMode,
  ThreadViewModeOverrideProvider,
  useInboxThreadViewMode,
} from "@/features/channels/lib/threadViewModePreference";
import { useInboxPanelOpen } from "@/features/home/lib/inboxPanelPreference";
import { OwnerViewLayout } from "@/features/agents/ownerView/OwnerViewLayout";

const AGENT_PUBKEY_RE = /^[0-9a-f]{64}$/i;

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
  /** Owner view: an agent of mine whose chats I am reading. */
  viewAs?: string;
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
    viewAs: nonEmptyString(search.viewAs),
  };
}

export const Route = createFileRoute("/channels/$channelId")({
  validateSearch: validateChannelSearch,
  component: ChannelRouteComponent,
});

const ChannelRouteScreen = LazyChannelRouteScreen;

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

  const ownerViewAgent =
    search.viewAs && AGENT_PUBKEY_RE.test(search.viewAs) && !isHuddleTranscript
      ? search.viewAs.toLowerCase()
      : null;
  if (ownerViewAgent) {
    // Owner view: my agent's chats without me, read-only.
    return (
      <div
        className="flex min-h-0 min-w-0 flex-1 overflow-hidden"
        data-testid="channel-route-layout"
      >
        <OwnerViewLayout
          agentPubkey={ownerViewAgent}
          channelId={channelId}
          openRootId={search.thread ?? null}
          renderChat={(chat) => (
            <React.Suspense
              fallback={<ViewLoadingFallback includeHeader kind="channel" />}
            >
              <ChannelRouteScreen
                autoSendDraftKey={null}
                channelId={channelId}
                ownerView={chat}
                searchHighlight={searchHighlight}
                selectedPostId={null}
                targetMessageId={search.messageId ?? null}
                targetReplyId={null}
                targetThreadRootId={
                  search.threadRootId ?? search.thread ?? null
                }
              />
            </React.Suspense>
          )}
        />
      </div>
    );
  }

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
        <ChannelRouteScreen
          autoSendDraftKey={search.autoSend ?? null}
          channelId={channelId}
          searchHighlight={searchHighlight}
          selectedPostId={null}
          targetMessageId={search.messageId ?? null}
          targetReplyId={null}
          targetThreadRootId={search.threadRootId ?? search.thread ?? null}
        />
      </ThreadViewModeOverrideProvider>
    </React.Suspense>
  );

  // The inbox list, when pulled out, lives in the left list column.
  return (
    <div
      className="flex min-h-0 min-w-0 flex-1 overflow-hidden"
      data-testid="channel-route-layout"
    >
      <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">
        {channelScreen}
      </div>
    </div>
  );
}
