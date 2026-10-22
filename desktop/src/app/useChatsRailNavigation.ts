import * as React from "react";

import { setSidebarListTab } from "@/features/sidebar/lib/sidebarListTab";
import { hasPendingWelcomeChannel } from "@/features/onboarding/welcome";

/**
 * Whether the home (Activity) screen should give way to Chats. Activity has
 * no rail item, so every way home — a fresh start, a reload, closing
 * Settings, hiding the open DM, switching account — lands on Chats instead.
 */
export function shouldLandOnChats({
  selectedView,
  pendingWelcome,
}: {
  selectedView: string;
  pendingWelcome: boolean;
}): boolean {
  return selectedView === "home" && !pendingWelcome;
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
  const lastRef = React.useRef<{
    channelId: string;
    threadRootId: string | null;
  } | null>(null);
  // Home lands on Chats: the room last open in this session, else the first
  // sidebar chat. With no chat at all, home stays.
  React.useEffect(() => {
    if (!channelsLoaded) return;
    if (
      !shouldLandOnChats({
        selectedView,
        pendingWelcome: hasPendingWelcomeChannel(),
      })
    ) {
      return;
    }
    const last = lastRef.current;
    const target = resolveChatsRailTarget(
      last?.channelId ?? null,
      sidebarChannels.map((channel) => channel.id),
    );
    if (!target) return;
    const thread =
      last?.channelId === target ? (last.threadRootId ?? undefined) : undefined;
    setSidebarListTab("chats");
    void goChannel(target, { replace: true, ...(thread ? { thread } : {}) });
  }, [channelsLoaded, goChannel, selectedView, sidebarChannels]);
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
