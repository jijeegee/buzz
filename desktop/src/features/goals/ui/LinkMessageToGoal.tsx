import * as React from "react";
import { Target } from "lucide-react";
import { toast } from "sonner";

import { getThreadReference } from "@/features/messages/lib/threading";
import type { TimelineMessage } from "@/features/messages/types";
import { useChannelReference } from "@/features/channels/openChannelDirectory";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui/dialog";
import { DropdownMenuItem } from "@/shared/ui/dropdown-menu";
import { goalForThread, goalRoot, messageGoalLinkScope } from "../goalTree";
import { useApplyGoalOpMutation, useGoalTreeQuery } from "../hooks";
import type { Apply } from "./GoalRows";
import { GoalLinkPicker } from "./ThreadGoalPanelView";

/** "Link to goal" in a message's More menu, while the room has goals on. */
export function LinkToGoalMenuItem({
  channelId,
  message,
  onSelect,
}: {
  channelId: string;
  message: TimelineMessage;
  onSelect: () => void;
}) {
  const channel = useChannelReference(channelId);
  if (channel?.goalsEnabled !== true) return null;
  return (
    <DropdownMenuItem
      data-testid={`link-message-to-goal-${message.id}`}
      onClick={onSelect}
    >
      <Target className="h-4 w-4" />
      Link to goal
    </DropdownMenuItem>
  );
}

/**
 * The thread goal picker for one message (see `messageGoalLinkScope`): a
 * message in a thread may link the thread's goal or its sub-goals, and a
 * main-timeline message any goal at layer 2 or below.
 */
export function LinkMessageToGoalDialog({
  channelId,
  message,
  onClose,
}: {
  channelId: string;
  message: TimelineMessage;
  onClose: () => void;
}) {
  const tree = useGoalTreeQuery(channelId).data?.tree;
  const applyGoalOp = useApplyGoalOpMutation(channelId).mutateAsync;
  // The picker closes before the edit lands, so failures surface as toasts.
  const apply = React.useCallback<Apply>(
    (op) =>
      applyGoalOp(op).catch((error: unknown) => {
        toast.error(
          error instanceof Error ? error.message : "Couldn't link to the goal",
        );
        throw error;
      }),
    [applyGoalOp],
  );
  const threadRootId = getThreadReference(message.tags ?? []).rootId;

  return (
    <Dialog onOpenChange={(open) => !open && onClose()} open>
      <DialogContent aria-describedby={undefined} className="sm:max-w-sm">
        <DialogHeader>
          <DialogTitle>Link to goal</DialogTitle>
        </DialogHeader>
        {!tree ? null : goalRoot(tree) ? (
          <div className="max-h-96 overflow-y-auto">
            <GoalLinkPicker
              apply={apply}
              linkedGoalId={goalForThread(tree, message.id)?.id ?? null}
              onDone={onClose}
              scope={messageGoalLinkScope(tree, threadRootId)}
              subject="message"
              threadRootId={message.id}
              tree={tree}
            />
          </div>
        ) : (
          <p className="text-sm text-muted-foreground">
            This conversation has no goals yet.
          </p>
        )}
      </DialogContent>
    </Dialog>
  );
}
