import * as React from "react";
import { Target } from "lucide-react";

import { Popover, PopoverContent, PopoverTrigger } from "@/shared/ui/popover";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/shared/ui/tooltip";
import { goalRoot } from "../goalTree";
import {
  useApplyGoalOpMutation,
  useGoalAssigneeNames,
  useGoalTreeQuery,
} from "../hooks";
import type { Apply } from "./GoalRows";
import { ThreadGoalPanelView } from "./ThreadGoalPanelView";

/**
 * Channel/DM header chip: always shows the layer 1 goal; opens the goals
 * panel. With no goal yet it offers to set one.
 */
export function GoalHeaderChip({
  channelId,
  compact = false,
  onOpenGoals,
}: {
  channelId: string;
  compact?: boolean;
  onOpenGoals: () => void;
}) {
  const goalQuery = useGoalTreeQuery(channelId);
  const root = goalQuery.data ? goalRoot(goalQuery.data.tree) : null;
  const label = root ? root.title : "Set a goal";

  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          aria-label={
            root ? `Goal: ${root.title}` : "Set a goal for this conversation"
          }
          className={`inline-flex h-7 ${compact ? "max-w-[8rem]" : "max-w-[16rem]"} items-center gap-1.5 rounded-full border border-border/70 bg-background/60 px-2.5 text-xs text-foreground transition-colors hover:bg-muted`}
          data-testid="goal-header-chip"
          onClick={onOpenGoals}
          type="button"
        >
          <Target className="h-3.5 w-3.5 shrink-0 text-primary" />
          <span className={`truncate ${root ? "" : "text-muted-foreground"}`}>
            {label}
          </span>
        </button>
      </TooltipTrigger>
      <TooltipContent>{root ? root.title : "Goals"}</TooltipContent>
    </Tooltip>
  );
}

/**
 * Thread head goal panel: the goal this thread works on with its sub-goals,
 * or a picker to link one. See `ThreadGoalPanelView`.
 */
export function ThreadGoalPanel({
  channelId,
  defaultExpanded,
  onOpenThread,
  threadRootId,
}: {
  channelId: string;
  defaultExpanded?: boolean;
  onOpenThread?: (threadRootId: string) => void;
  threadRootId: string;
}) {
  const goalQuery = useGoalTreeQuery(channelId);
  const applyMutation = useApplyGoalOpMutation(channelId);
  const tree = goalQuery.data?.tree;
  const nameOf = useGoalAssigneeNames(tree);
  const applyGoalOp = applyMutation.mutateAsync;
  const apply = React.useCallback<Apply>(
    (op) => applyGoalOp(op),
    [applyGoalOp],
  );
  if (!tree) return null;
  return (
    <ThreadGoalPanelView
      apply={apply}
      defaultExpanded={defaultExpanded}
      error={
        applyMutation.error instanceof Error
          ? applyMutation.error.message
          : null
      }
      nameOf={nameOf}
      onOpenThread={onOpenThread}
      threadRootId={threadRootId}
      tree={tree}
    />
  );
}

/**
 * Thread header chip: the channel header's goal chip, in the thread title
 * bar. It opens the whole goal tree in place, with this thread's goal branch
 * editable (see `ThreadGoalPanelView`), and leaves the thread open.
 */
export function ThreadGoalHeaderChip({
  channelId,
  onOpenThread,
  threadRootId,
}: {
  channelId: string;
  /** Go to another goal's thread (closes the popover). */
  onOpenThread?: (threadRootId: string) => void;
  threadRootId: string;
}) {
  const goalQuery = useGoalTreeQuery(channelId);
  const root = goalQuery.data ? goalRoot(goalQuery.data.tree) : null;
  const [open, setOpen] = React.useState(false);
  const openThread = React.useMemo(
    () =>
      onOpenThread
        ? (thread: string) => {
            setOpen(false);
            onOpenThread(thread);
          }
        : undefined,
    [onOpenThread],
  );
  if (!root) return null;
  return (
    <Popover onOpenChange={setOpen} open={open}>
      <PopoverTrigger asChild>
        <button
          aria-label={`Goal: ${root.title}`}
          className="inline-flex h-7 max-w-[10rem] items-center gap-1.5 rounded-full border border-border/70 bg-background/60 px-2.5 text-xs text-foreground transition-colors hover:bg-muted"
          data-testid="thread-goal-header-chip"
          title={root.title}
          type="button"
        >
          <Target className="h-3.5 w-3.5 shrink-0 text-primary" />
          <span className="truncate">{root.title}</span>
        </button>
      </PopoverTrigger>
      <PopoverContent align="end" className="w-[28rem] max-w-[90vw] p-0 pt-2">
        <ThreadGoalPanel
          channelId={channelId}
          defaultExpanded
          onOpenThread={openThread}
          threadRootId={threadRootId}
        />
      </PopoverContent>
    </Popover>
  );
}
