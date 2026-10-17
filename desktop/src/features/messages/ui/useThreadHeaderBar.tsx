import * as React from "react";

import { useAppShell } from "@/app/AppShellContext";
import { threadGoalRootId } from "@/features/goals/goalTree";
import { threadCloseState } from "@/features/messages/lib/taskThread";
import type { TimelineMessage } from "@/features/messages/types";
import { useThreadCloseToggle } from "@/features/messages/useThreadCloseToggle";
import { ThreadClosedLine } from "./ThreadClosedLine";
import { ThreadHeaderActions } from "./ThreadHeaderActions";

/**
 * The thread header bar's buttons and the closed line under the replies.
 * Close/Reopen writes the marker replies old apps already read, so it does
 * not wait for the relay's chat state; Mark as unread uses the chat state
 * while it is on and otherwise marks unread as the message menu does today.
 */
export function useThreadHeaderBar({
  channelId,
  currentPubkey,
  disabled,
  isFollowingThread,
  isHuddleTranscript,
  onFollowThread,
  onMarkUnread,
  onUnfollowThread,
  threadHead,
  threadMessages,
}: {
  channelId: string | null;
  currentPubkey?: string;
  disabled?: boolean;
  isFollowingThread?: boolean;
  isHuddleTranscript: boolean;
  onFollowThread?: () => void;
  onMarkUnread?: (message: TimelineMessage) => void;
  onUnfollowThread?: () => void;
  threadHead: TimelineMessage | null;
  threadMessages: readonly TimelineMessage[];
}): {
  closedLine: React.ReactNode;
  /** The bar, around the panel's existing Goals chip and Show in channel. */
  renderHeaderActions: (
    goalChip: React.ReactNode,
    showInChannelButton: React.ReactNode,
  ) => React.ReactNode;
  isClosed: boolean;
} {
  const { markThreadUnread } = useAppShell();
  const { pending, setThreadClosed } = useThreadCloseToggle();
  const closeState = React.useMemo(
    () =>
      isHuddleTranscript
        ? { closed: false, closedBy: null, marker: null }
        : threadCloseState(threadMessages),
    [isHuddleTranscript, threadMessages],
  );
  if (!threadHead) {
    return {
      closedLine: null,
      isClosed: false,
      renderHeaderActions: () => null,
    };
  }
  const rootId = threadGoalRootId(threadHead);
  const canToggle =
    !isHuddleTranscript && Boolean(channelId) && !threadHead.pending;
  const toggleClosed =
    canToggle && channelId && !disabled
      ? (closed: boolean) => setThreadClosed({ channelId, rootId, closed })
      : undefined;
  const closerName =
    closeState.closedBy &&
    currentPubkey &&
    closeState.closedBy.toLowerCase() === currentPubkey.toLowerCase()
      ? "You"
      : (closeState.marker?.author ?? null);

  const renderHeaderActions = (
    goalChip: React.ReactNode,
    showInChannelButton: React.ReactNode,
  ) =>
    isHuddleTranscript ? (
      goalChip || showInChannelButton ? (
        <>
          {goalChip}
          {showInChannelButton}
        </>
      ) : null
    ) : (
      <ThreadHeaderActions
        channelId={threadHead.pending ? null : channelId}
        closePending={pending}
        goalChip={goalChip}
        isClosed={closeState.closed}
        isFollowing={isFollowingThread}
        onFollow={onFollowThread}
        onMarkUnread={
          onMarkUnread
            ? () => {
                if (!markThreadUnread(rootId)) onMarkUnread(threadHead);
              }
            : undefined
        }
        onToggleClosed={
          toggleClosed ? () => toggleClosed(!closeState.closed) : undefined
        }
        onUnfollow={onUnfollowThread}
        rootId={rootId}
        showInChannelButton={showInChannelButton}
      />
    );

  const closedLine = closeState.closed ? (
    <ThreadClosedLine
      closerName={closerName}
      onReopen={toggleClosed ? () => toggleClosed(false) : undefined}
      reopenPending={pending}
    />
  ) : null;

  return { closedLine, isClosed: closeState.closed, renderHeaderActions };
}
