import * as React from "react";

import { setInboxPanelOpen } from "@/features/home/lib/inboxPanelPreference";
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
 * Where the rail's Chats item goes: the room last open in this session, else
 * the first sidebar chat. On a channel it only hides the Activity panel, so
 * the list beside the room is the sidebar's chat list again.
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
  selectedView,
  sidebarChannels,
}: {
  /** The channel list has loaded once, so an empty list really is empty. */
  channelsLoaded: boolean;
  goChannel: (channelId: string, options?: { replace?: boolean }) => unknown;
  selectedChannelId: string | null;
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
  const lastChannelIdRef = React.useRef<string | null>(null);
  React.useEffect(() => {
    if (selectedView === "channel" && selectedChannelId) {
      lastChannelIdRef.current = selectedChannelId;
    }
  }, [selectedChannelId, selectedView]);
  return React.useCallback(() => {
    setInboxPanelOpen(false);
    if (selectedView === "channel") return;
    const target = resolveChatsRailTarget(
      lastChannelIdRef.current,
      sidebarChannels.map((channel) => channel.id),
    );
    if (target) void goChannel(target);
  }, [goChannel, selectedView, sidebarChannels]);
}
