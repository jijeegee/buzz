import * as React from "react";

import { setInboxPanelOpen } from "@/features/home/lib/inboxPanelPreference";
import { setSidebarListTab } from "@/features/sidebar/lib/sidebarListTab";
import { hasPendingWelcomeChannel } from "@/features/onboarding/welcome";

/**
 * The app was started without a route (no hash), as on a fresh launch. A
 * reload keeps its hash, so it returns to the screen it was on.
 */
const STARTED_WITHOUT_ROUTE =
  typeof window === "undefined" ||
  window.location.hash === "" ||
  window.location.hash === "#";
let landingDone = false;

/** Whether a fresh start should land on Chats instead of Activity. */
export function shouldLandOnChats({
  startedWithoutRoute,
  selectedView,
  pendingWelcome,
}: {
  startedWithoutRoute: boolean;
  selectedView: string;
  pendingWelcome: boolean;
}): boolean {
  return startedWithoutRoute && selectedView === "home" && !pendingWelcome;
}

/**
 * Where the rail's Chats item goes: the room (and thread) last open in this
 * session, else the first sidebar chat. On a channel it only switches the
 * left list back to Chats.
 */
export function resolveChatsRailTarget(
  lastChannelId: string | null,
  sidebarChannelIds: readonly string[],
): string | null {
  if (lastChannelId && sidebarChannelIds.includes(lastChannelId)) {
    return lastChannelId;
  }
  return sidebarChannelIds[0] ?? null;
}

export function useChatsRailNavigation({
  channelsLoaded,
  goChannel,
  selectedChannelId,
  selectedThreadRootId,
  selectedView,
  sidebarChannels,
}: {
  /** The channel list has loaded once, so an empty list really is empty. */
  channelsLoaded: boolean;
  goChannel: (
    channelId: string,
    options?: { replace?: boolean; thread?: string },
  ) => unknown;
  selectedChannelId: string | null;
  /** The thread open beside the room, if any. */
  selectedThreadRootId: string | null;
  selectedView: string;
  sidebarChannels: readonly { id: string }[];
}): () => void {
  // A fresh start lands on Chats (the first sidebar chat), once per launch.
  React.useEffect(() => {
    if (landingDone || !channelsLoaded) return;
    landingDone = true;
    if (
      !shouldLandOnChats({
        startedWithoutRoute: STARTED_WITHOUT_ROUTE,
        selectedView,
        pendingWelcome: hasPendingWelcomeChannel(),
      })
    ) {
      return;
    }
    const target = resolveChatsRailTarget(
      null,
      sidebarChannels.map((channel) => channel.id),
    );
    if (!target) return;
    setInboxPanelOpen(false);
    void goChannel(target, { replace: true });
  }, [channelsLoaded, goChannel, selectedView, sidebarChannels]);
  const lastRef = React.useRef<{
    channelId: string;
    threadRootId: string | null;
  } | null>(null);
  React.useEffect(() => {
    if (selectedView === "channel" && selectedChannelId) {
      lastRef.current = {
        channelId: selectedChannelId,
        threadRootId: selectedThreadRootId,
      };
    }
  }, [selectedChannelId, selectedThreadRootId, selectedView]);
  return React.useCallback(() => {
    setSidebarListTab("chats");
    if (selectedView === "channel") return;
    const last = lastRef.current;
    const target = resolveChatsRailTarget(
      last?.channelId ?? null,
      sidebarChannels.map((channel) => channel.id),
    );
    if (!target) return;
    // Back to the thread that was open there, not just its channel.
    const thread =
      last?.channelId === target ? (last.threadRootId ?? undefined) : undefined;
    void goChannel(target, thread ? { thread } : undefined);
  }, [goChannel, selectedView, sidebarChannels]);
}
