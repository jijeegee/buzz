import * as React from "react";

import { useAppShellRoom } from "@/app/AppShellContext";
import { threadGoalRootId } from "@/features/goals/goalTree";
import type { MainTimelineEntry } from "@/features/messages/lib/threadPanel";
import type { TimelineMessage } from "@/features/messages/types";
import { useAppFocused } from "@/shared/lib/useDocumentVisible";

/** Newest createdAt (unix seconds) among the thread root and shown replies. */
export function newestShownThreadAt(
  head: Pick<TimelineMessage, "createdAt"> | null | undefined,
  replies: readonly Pick<MainTimelineEntry, "message">[],
): number | null {
  if (!head) return null;
  let newest = head.createdAt;
  for (const entry of replies) {
    if (entry.message.createdAt > newest) newest = entry.message.createdAt;
  }
  return newest;
}

/**
 * An open thread panel in a visible, focused window marks the thread read in
 * the relay's chat state, up to the newest message shown. Only the thread;
 * the channel's main timeline keeps its own read position. A no-op while chat
 * state is off, so local read markers behave as before.
 */
export function useThreadOpenServerRead(
  head: Pick<TimelineMessage, "createdAt" | "id" | "rootId"> | null | undefined,
  replies: readonly Pick<MainTimelineEntry, "message">[],
) {
  const rootId = head ? threadGoalRootId(head) : null;
  const newestAt = newestShownThreadAt(head, replies);
  const { mirrorThreadRead } = useAppShellRoom();
  const focused = useAppFocused();
  React.useEffect(() => {
    if (!rootId || newestAt === null || !focused) return;
    mirrorThreadRead(rootId, newestAt);
  }, [focused, mirrorThreadRead, newestAt, rootId]);
}
