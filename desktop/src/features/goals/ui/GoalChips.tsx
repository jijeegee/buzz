import * as React from "react";
import { Link2, Target, Unlink } from "lucide-react";

import { Popover, PopoverContent, PopoverTrigger } from "@/shared/ui/popover";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/shared/ui/tooltip";
import { goalForThread, goalOutline, goalPath, goalRoot } from "../goalTree";
import { useApplyGoalOpMutation, useGoalTreeQuery } from "../hooks";

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
 * Thread chip: the goal path this thread works on, or a picker to link it.
 */
export function ThreadGoalChip({
  channelId,
  threadRootId,
}: {
  channelId: string;
  threadRootId: string;
}) {
  const goalQuery = useGoalTreeQuery(channelId);
  const applyMutation = useApplyGoalOpMutation(channelId);
  const [open, setOpen] = React.useState(false);
  const tree = goalQuery.data?.tree;
  if (!tree || !goalRoot(tree)) return null;

  const goal = goalForThread(tree, threadRootId);
  const path = goal ? goalPath(tree, goal.id) : [];
  // The layer 1 goal is already in the channel header; show the rest.
  const shown = path.length > 1 ? path.slice(1) : path;

  return (
    <div
      className="flex min-w-0 items-center gap-1.5 px-4 pb-2 text-xs"
      data-testid="thread-goal-chip"
    >
      <Target className="h-3.5 w-3.5 shrink-0 text-primary" />
      <Popover onOpenChange={setOpen} open={open}>
        <PopoverTrigger asChild>
          <button
            className="min-w-0 truncate rounded-md px-1 py-0.5 text-left hover:bg-muted"
            type="button"
          >
            {goal ? (
              <span className="text-foreground">
                {shown.map((node) => node.title).join(" › ")}
              </span>
            ) : (
              <span className="inline-flex items-center gap-1 text-muted-foreground">
                <Link2 className="h-3 w-3" />
                Link this thread to a goal
              </span>
            )}
          </button>
        </PopoverTrigger>
        <PopoverContent
          align="start"
          className="max-h-80 w-80 overflow-y-auto p-1"
        >
          {goalOutline(tree).map(({ layer, node }) => (
            <button
              className={`flex w-full items-start gap-1 rounded-md px-2 py-1 text-left text-xs hover:bg-muted ${node.id === goal?.id ? "bg-muted" : ""}`}
              data-testid={`thread-goal-option-${node.id}`}
              key={node.id}
              onClick={() => {
                setOpen(false);
                void applyMutation.mutateAsync({
                  op: "link",
                  id: node.id,
                  thread: threadRootId,
                });
              }}
              style={{ paddingLeft: `${0.5 + (layer - 1) * 0.75}rem` }}
              type="button"
            >
              <span className="text-muted-foreground">L{layer}</span>
              <span className="min-w-0 break-words">{node.title}</span>
            </button>
          ))}
          {goal ? (
            <button
              className="mt-1 flex w-full items-center gap-1 rounded-md border-t border-border/60 px-2 py-1.5 text-left text-xs text-muted-foreground hover:bg-muted"
              onClick={() => {
                setOpen(false);
                void applyMutation.mutateAsync({
                  op: "unlink",
                  thread: threadRootId,
                });
              }}
              type="button"
            >
              <Unlink className="h-3 w-3" />
              Unlink from goal
            </button>
          ) : null}
        </PopoverContent>
      </Popover>
    </div>
  );
}
