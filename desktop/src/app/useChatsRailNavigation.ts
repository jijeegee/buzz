import * as React from "react";

import { setInboxPanelOpen } from "@/features/home/lib/inboxPanelPreference";

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
  goChannel,
  selectedChannelId,
  selectedView,
  sidebarChannels,
}: {
  goChannel: (channelId: string) => unknown;
  selectedChannelId: string | null;
  selectedView: string;
  sidebarChannels: readonly { id: string }[];
}): () => void {
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
