import * as React from "react";

import type { PanelValueSetter } from "@/features/channels/ui/useChannelPanelHistoryState";
import type { TimelineMessage } from "@/features/messages/types";

/**
 * Keeps thread-panel and edit-composer targets consistent with the messages
 * the panel shows: seeds the reply target from the open thread's root id,
 * points a reply target that no longer resolves back at the root, and clears
 * a stale edit target. The panel opens on the root id alone, so a missing
 * root never closes it (docs/message-sync.md#opening-a-thread-from-its-root-id);
 * only a panel that does not open that way (forum rooms, huddle transcripts)
 * still closes when its head is not loaded.
 */
export function useThreadTargetSync({
  clearOptimisticThreadOverride,
  editTargetId,
  editTargetMessage,
  isTimelineLoading,
  openThreadHeadId,
  opensOnRootId,
  setEditTargetId,
  setExpandedThreadReplyIds,
  setOpenThreadHeadId,
  setThreadReplyTargetId,
  setThreadScrollTargetId,
  threadHeadId,
  threadHeadMessage,
  threadReplyTargetId,
  threadReplyTargetMessage,
}: {
  clearOptimisticThreadOverride: () => void;
  editTargetId: string | null;
  editTargetMessage: TimelineMessage | null;
  isTimelineLoading: boolean;
  /** The `thread` route value. */
  openThreadHeadId: string | null;
  /** The panel opens on the root id alone (placeholder head until found). */
  opensOnRootId: boolean;
  setEditTargetId: (id: string | null) => void;
  setExpandedThreadReplyIds: (ids: Set<string>) => void;
  setOpenThreadHeadId: PanelValueSetter;
  setThreadReplyTargetId: (id: string | null) => void;
  setThreadScrollTargetId: (id: string | null) => void;
  /** The thread the panel shows (the route's, or a huddle's own). */
  threadHeadId: string | null;
  /** The panel's head as shown, placeholder or deleted row included. */
  threadHeadMessage: TimelineMessage | null;
  threadReplyTargetId: string | null;
  /** The panel's reply target as shown. */
  threadReplyTargetMessage: TimelineMessage | null;
}) {
  React.useEffect(() => {
    if (!opensOnRootId && openThreadHeadId && !threadHeadMessage) {
      if (isTimelineLoading) {
        return;
      }
      clearOptimisticThreadOverride();
      setOpenThreadHeadId(null, { replace: true });
      setExpandedThreadReplyIds(new Set());
      setThreadScrollTargetId(null);
      return;
    }

    const headId = threadHeadMessage?.id ?? (opensOnRootId ? threadHeadId : null);
    if (headId && !threadReplyTargetId) {
      setThreadReplyTargetId(headId);
      return;
    }

    if (
      threadReplyTargetId &&
      !threadReplyTargetMessage &&
      threadReplyTargetId !== headId
    ) {
      setThreadReplyTargetId(headId);
    }
    if (editTargetId && !editTargetMessage) {
      setEditTargetId(null);
    }
  }, [
    clearOptimisticThreadOverride,
    editTargetId,
    editTargetMessage,
    isTimelineLoading,
    openThreadHeadId,
    opensOnRootId,
    setEditTargetId,
    setExpandedThreadReplyIds,
    setOpenThreadHeadId,
    setThreadReplyTargetId,
    setThreadScrollTargetId,
    threadHeadId,
    threadHeadMessage,
    threadReplyTargetId,
    threadReplyTargetMessage,
  ]);
}
