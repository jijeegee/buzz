import * as React from "react";
import { History, Pencil, Target } from "lucide-react";

import { useUsersBatchQuery } from "@/features/profile/hooks";
import type { GoalNode, GoalTree } from "@/shared/api/tauriGoals";
import { newGoalId } from "@/shared/api/tauriGoals";
import { Button } from "@/shared/ui/button";
import { goalChildren, goalProgress, goalRoot } from "../goalTree";
import {
  useApplyGoalOpMutation,
  useGoalAssigneeNames,
  useGoalHistoryQuery,
  useGoalTreeQuery,
  useRestoreGoalTreeMutation,
} from "../hooks";
import { AddGoalInline, type Apply, GoalEditor, GoalRow } from "./GoalRows";

type GoalsPanelProps = {
  channelId: string;
  onOpenThread?: (threadRootId: string) => void;
};

/** The goal tree of a channel or DM: layer 1 card, outline, history. */
export function GoalsPanel({ channelId, onOpenThread }: GoalsPanelProps) {
  const goalQuery = useGoalTreeQuery(channelId);
  const applyMutation = useApplyGoalOpMutation(channelId);
  const [showHistory, setShowHistory] = React.useState(false);
  const tree = goalQuery.data?.tree ?? { v: 1, nodes: [] };
  const root = goalRoot(tree);
  const nameOf = useGoalAssigneeNames(goalQuery.data?.tree);

  const applyGoalOp = applyMutation.mutateAsync;
  const apply = React.useCallback<Apply>(
    (op) => applyGoalOp(op),
    [applyGoalOp],
  );

  if (goalQuery.isLoading) {
    return <p className="pt-4 text-sm text-muted-foreground">Loading goals…</p>;
  }
  if (goalQuery.error instanceof Error) {
    return (
      <p className="mt-4 rounded-xl border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive">
        {goalQuery.error.message}
      </p>
    );
  }

  return (
    <div className="space-y-4 pt-3" data-testid="goals-panel">
      {root ? (
        <RootCard apply={apply} root={root} tree={tree} />
      ) : (
        <SetRootForm apply={apply} />
      )}

      {root ? (
        <div className="space-y-1" data-testid="goals-outline">
          {goalChildren(tree, root.id).map((child) => (
            <GoalRow
              apply={apply}
              key={child.id}
              layer={2}
              nameOf={nameOf}
              node={child}
              onOpenThread={onOpenThread}
              tree={tree}
            />
          ))}
          <AddGoalInline
            apply={apply}
            label="Add a layer 2 goal"
            parentId={root.id}
          />
        </div>
      ) : null}

      {applyMutation.error instanceof Error ? (
        <p className="rounded-xl border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive">
          {applyMutation.error.message}
        </p>
      ) : null}

      <div className="border-t border-border/60 pt-3">
        <Button
          data-testid="goals-history-toggle"
          onClick={() => setShowHistory((open) => !open)}
          size="sm"
          type="button"
          variant="ghost"
        >
          <History className="mr-1.5 h-3.5 w-3.5" />
          {showHistory ? "Hide history" : "History"}
        </Button>
        {showHistory ? <GoalHistory channelId={channelId} /> : null}
      </div>
    </div>
  );
}

function RootCard({
  apply,
  root,
  tree,
}: {
  apply: Apply;
  root: GoalNode;
  tree: GoalTree;
}) {
  const [editing, setEditing] = React.useState(false);
  const { done, total } = goalProgress(tree, root.id);
  const percent = total > 0 ? Math.round((done / total) * 100) : 0;

  if (editing) {
    return (
      <GoalEditor
        initialNote={root.note ?? ""}
        initialTitle={root.title}
        onCancel={() => setEditing(false)}
        onSave={async (title, note) => {
          await apply({ op: "set_root", id: root.id, title, note });
          setEditing(false);
        }}
        titleLabel="Layer 1 goal (one sentence)"
      />
    );
  }

  return (
    <section
      className="rounded-2xl border border-border/70 bg-muted/30 p-4"
      data-testid="goals-root-card"
    >
      <div className="flex items-start gap-2">
        <Target className="mt-0.5 h-4 w-4 shrink-0 text-primary" />
        <div className="min-w-0 flex-1">
          <p className="text-xs font-medium uppercase tracking-wide text-muted-foreground">
            Layer 1 goal
          </p>
          <p className="mt-1 break-words text-sm font-semibold text-foreground">
            {root.title}
          </p>
        </div>
        <Button
          aria-label="Edit layer 1 goal"
          data-testid="goals-root-edit"
          onClick={() => setEditing(true)}
          size="icon"
          type="button"
          variant="ghost"
        >
          <Pencil className="h-3.5 w-3.5" />
        </Button>
      </div>
      {root.note ? (
        <p className="mt-3 whitespace-pre-wrap break-words text-sm text-muted-foreground">
          {root.note}
        </p>
      ) : null}
      {total > 0 ? (
        <div className="mt-3">
          <div className="h-1.5 overflow-hidden rounded-full bg-muted">
            <div
              className="h-full rounded-full bg-primary transition-all"
              style={{ width: `${percent}%` }}
            />
          </div>
          <p className="mt-1 text-xs text-muted-foreground">
            {done} of {total} goals done
          </p>
        </div>
      ) : null}
    </section>
  );
}

function SetRootForm({ apply }: { apply: Apply }) {
  return (
    <section className="space-y-2" data-testid="goals-empty">
      <p className="text-sm text-muted-foreground">
        Give this conversation one top goal. Split it into smaller goals below,
        and agents working here will see where their work fits.
      </p>
      <GoalEditor
        initialNote=""
        initialTitle=""
        onSave={(title, note) =>
          apply({ op: "set_root", id: newGoalId(), title, note })
        }
        submitLabel="Set goal"
        titleLabel="Layer 1 goal (one sentence)"
      />
    </section>
  );
}

function GoalHistory({ channelId }: { channelId: string }) {
  const historyQuery = useGoalHistoryQuery(channelId, true);
  const restoreMutation = useRestoreGoalTreeMutation(channelId);
  const revisions = historyQuery.data ?? [];
  const authors = React.useMemo(
    () => [...new Set(revisions.map((r) => r.author))],
    [revisions],
  );
  const usersQuery = useUsersBatchQuery(authors);

  if (historyQuery.isLoading) {
    return (
      <p className="mt-2 text-xs text-muted-foreground">Loading history…</p>
    );
  }
  return (
    <ul className="mt-2 space-y-1" data-testid="goals-history">
      {revisions.map((revision, index) => (
        <li
          className="flex items-center gap-2 rounded-lg px-2 py-1 text-xs hover:bg-muted/40"
          key={revision.revision}
        >
          <span className="flex-1 text-muted-foreground">
            {new Date(revision.createdAt * 1000).toLocaleString()} ·{" "}
            {usersQuery.data?.profiles[revision.author]?.displayName ??
              "Unknown member"}{" "}
            · {revision.goals ?? "?"} goals
          </span>
          {index === 0 ? (
            <span className="text-muted-foreground">Current</span>
          ) : (
            <Button
              disabled={restoreMutation.isPending}
              onClick={() => restoreMutation.mutate(revision.revision)}
              size="sm"
              type="button"
              variant="ghost"
            >
              Restore
            </Button>
          )}
        </li>
      ))}
    </ul>
  );
}
