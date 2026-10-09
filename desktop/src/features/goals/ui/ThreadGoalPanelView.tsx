import * as React from "react";
import {
  ChevronDown,
  ChevronRight,
  Link2,
  Plus,
  Target,
  Unlink,
} from "lucide-react";

import type { GoalNode, GoalTree } from "@/shared/api/tauriGoals";
import { newGoalId } from "@/shared/api/tauriGoals";
import { Popover, PopoverContent, PopoverTrigger } from "@/shared/ui/popover";
import {
  goalChildren,
  goalForThread,
  goalOutline,
  goalPath,
  goalProgress,
  goalRoot,
} from "../goalTree";
import {
  AddGoalInline,
  type Apply,
  GoalEditor,
  GoalRow,
  GoalStatusButton,
  runGoalEdit,
} from "./GoalRows";

type ThreadGoalPanelViewProps = {
  apply: Apply;
  /** Message of the last failed edit, if any. */
  error?: string | null;
  nameOf: (pubkey: string) => string;
  threadRootId: string;
  tree: GoalTree;
};

/**
 * Thread head goal panel. Linked: a one-line summary (status, path below
 * layer 1, sub-goal progress) that expands into the goal's sub-goal tree,
 * where sub-goals can be added and checked off. Unlinked: a picker that links
 * the thread to a goal, or adds a new goal and links it in one revision.
 * Renders nothing when the conversation has no goals.
 */
export function ThreadGoalPanelView({
  apply,
  error,
  nameOf,
  threadRootId,
  tree,
}: ThreadGoalPanelViewProps) {
  const [expanded, setExpanded] = React.useState(false);
  const [pickerOpen, setPickerOpen] = React.useState(false);
  const subtreeId = React.useId();
  if (!goalRoot(tree)) return null;

  const goal = goalForThread(tree, threadRootId);
  const picker = (trigger: React.ReactNode) => (
    <Popover onOpenChange={setPickerOpen} open={pickerOpen}>
      <PopoverTrigger asChild>{trigger}</PopoverTrigger>
      <PopoverContent
        align="start"
        className="max-h-96 w-80 overflow-y-auto p-1"
      >
        <GoalLinkPicker
          apply={apply}
          linkedGoalId={goal?.id ?? null}
          onDone={() => setPickerOpen(false)}
          threadRootId={threadRootId}
          tree={tree}
        />
      </PopoverContent>
    </Popover>
  );
  const errorLine = error ? (
    <p className="px-2 pb-1 text-xs text-destructive" role="alert">
      {error}
    </p>
  ) : null;

  if (!goal) {
    return (
      <div className="px-4 pb-2" data-testid="thread-goal-chip">
        <div className="flex min-w-0 items-center gap-1.5 text-xs">
          <Target className="h-3.5 w-3.5 shrink-0 text-primary" />
          {picker(
            <button
              className="inline-flex min-w-0 items-center gap-1 truncate rounded-md px-1 py-0.5 text-left text-muted-foreground hover:bg-muted"
              data-testid="thread-goal-link"
              type="button"
            >
              <Link2 className="h-3 w-3" />
              Link this thread to a goal
            </button>,
          )}
        </div>
        {errorLine}
      </div>
    );
  }

  const path = goalPath(tree, goal.id);
  const layer = path.length;
  // The layer 1 goal is already in the channel header; show the rest.
  const shown = path.length > 1 ? path.slice(1) : path;
  const { done, total } = goalProgress(tree, goal.id);
  const closed = goal.status === "done" || goal.status === "dropped";

  return (
    <section
      aria-label="Thread goal"
      className="mx-4 mb-2 rounded-xl border border-border/60 bg-muted/20 text-xs"
      data-testid="thread-goal-panel"
    >
      <div className="flex min-w-0 items-center gap-1.5 px-2 py-1">
        <Target className="h-3.5 w-3.5 shrink-0 text-primary" />
        <GoalStatusButton apply={apply} className="mt-0" node={goal} />
        <button
          aria-controls={subtreeId}
          aria-expanded={expanded}
          className="flex min-w-0 flex-1 items-center gap-1 rounded-md px-1 py-0.5 text-left hover:bg-muted"
          data-testid="thread-goal-toggle"
          onClick={() => setExpanded((value) => !value)}
          type="button"
        >
          {expanded ? (
            <ChevronDown className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
          ) : (
            <ChevronRight className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
          )}
          <span
            className={`min-w-0 truncate ${closed ? "text-muted-foreground line-through" : "text-foreground"}`}
          >
            {shown.map((node) => node.title).join(" › ")}
          </span>
          {total > 0 ? (
            <span
              className="shrink-0 text-muted-foreground"
              data-testid="thread-goal-progress"
            >
              {done}/{total}
            </span>
          ) : null}
        </button>
        {picker(
          <button
            aria-label="Change or unlink this thread's goal"
            className="flex h-6 w-6 shrink-0 items-center justify-center rounded-md text-muted-foreground hover:bg-muted hover:text-foreground"
            data-testid="thread-goal-link"
            type="button"
          >
            <Link2 className="h-3.5 w-3.5" />
          </button>,
        )}
      </div>
      {expanded ? (
        <div
          className="max-h-64 overflow-y-auto border-t border-border/50 px-2 py-1"
          data-testid="thread-goal-subtree"
          id={subtreeId}
        >
          {goal.note ? (
            <p className="whitespace-pre-wrap break-words px-1 pb-1 text-muted-foreground">
              {goal.note}
            </p>
          ) : null}
          {goalChildren(tree, goal.id).map((child) => (
            <GoalRow
              apply={apply}
              key={child.id}
              layer={layer + 1}
              nameOf={nameOf}
              node={child}
              tree={tree}
            />
          ))}
          <AddGoalInline
            apply={apply}
            label={`Add a layer ${layer + 1} goal`}
            parentId={goal.id}
          />
        </div>
      ) : null}
      {errorLine}
    </section>
  );
}

/**
 * Pick the goal a thread works on: link to any goal, add a new goal under
 * any goal and link to it (one revision), or unlink.
 */
export function GoalLinkPicker({
  apply,
  linkedGoalId,
  onDone,
  threadRootId,
  tree,
}: {
  apply: Apply;
  linkedGoalId: string | null;
  onDone: () => void;
  threadRootId: string;
  tree: GoalTree;
}) {
  const [createUnder, setCreateUnder] = React.useState<GoalNode | null>(null);

  if (createUnder) {
    const layer = goalPath(tree, createUnder.id).length + 1;
    return (
      <div className="p-2" data-testid="thread-goal-create">
        <p className="mb-1 break-words text-xs text-muted-foreground">
          New layer {layer} goal under “{createUnder.title}”, linked to this
          thread
        </p>
        <GoalEditor
          initialNote=""
          initialTitle=""
          onCancel={() => setCreateUnder(null)}
          onSave={async (title, note) => {
            const id = newGoalId();
            await apply([
              { op: "add", id, parent: createUnder.id, title, note },
              { op: "link", id, thread: threadRootId },
            ]);
            onDone();
          }}
          submitLabel="Add and link"
          titleLabel="Goal (one sentence)"
        />
      </div>
    );
  }

  return (
    <div data-testid="thread-goal-picker">
      {goalOutline(tree).map(({ layer, node }) => (
        <div className="group flex items-start" key={node.id}>
          <button
            className={`flex min-w-0 flex-1 items-start gap-1 rounded-md px-2 py-1 text-left text-xs hover:bg-muted ${node.id === linkedGoalId ? "bg-muted" : ""}`}
            data-testid={`thread-goal-option-${node.id}`}
            onClick={() => {
              onDone();
              runGoalEdit(
                apply({ op: "link", id: node.id, thread: threadRootId }),
              );
            }}
            style={{ paddingLeft: `${0.5 + (layer - 1) * 0.75}rem` }}
            type="button"
          >
            <span className="text-muted-foreground">L{layer}</span>
            <span className="min-w-0 break-words">{node.title}</span>
          </button>
          <button
            aria-label={`Add a goal under “${node.title}” and link this thread to it`}
            className="flex h-6 w-6 shrink-0 items-center justify-center rounded-md text-muted-foreground opacity-60 hover:bg-muted hover:text-foreground focus-visible:opacity-100 group-hover:opacity-100"
            data-testid={`thread-goal-create-under-${node.id}`}
            onClick={() => setCreateUnder(node)}
            type="button"
          >
            <Plus className="h-3.5 w-3.5" />
          </button>
        </div>
      ))}
      {linkedGoalId ? (
        <button
          className="mt-1 flex w-full items-center gap-1 rounded-md border-t border-border/60 px-2 py-1.5 text-left text-xs text-muted-foreground hover:bg-muted"
          data-testid="thread-goal-unlink"
          onClick={() => {
            onDone();
            runGoalEdit(apply({ op: "unlink", thread: threadRootId }));
          }}
          type="button"
        >
          <Unlink className="h-3 w-3" />
          Unlink from goal
        </button>
      ) : null}
    </div>
  );
}
