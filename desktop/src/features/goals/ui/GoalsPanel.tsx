import * as React from "react";
import {
  ChevronDown,
  ChevronRight,
  History,
  MessagesSquare,
  Pencil,
  Plus,
  Target,
  Trash2,
} from "lucide-react";

import { useUsersBatchQuery } from "@/features/profile/hooks";
import type {
  GoalNode,
  GoalOp,
  GoalStatus,
  GoalTree,
} from "@/shared/api/tauriGoals";
import { newGoalId } from "@/shared/api/tauriGoals";
import { Button } from "@/shared/ui/button";
import { Input } from "@/shared/ui/input";
import { Textarea } from "@/shared/ui/textarea";
import {
  GOAL_STATUS_LABEL,
  goalChildren,
  goalProgress,
  goalRoot,
  goalSubtreeIds,
  goalTitleError,
} from "../goalTree";
import {
  useApplyGoalOpMutation,
  useGoalHistoryQuery,
  useGoalTreeQuery,
  useRestoreGoalTreeMutation,
} from "../hooks";

const STATUS_ORDER: GoalStatus[] = ["open", "in_progress", "done", "dropped"];

const STATUS_DOT: Record<GoalStatus, string> = {
  open: "border-muted-foreground/60",
  in_progress: "border-primary bg-primary/30",
  done: "border-emerald-500 bg-emerald-500",
  dropped: "border-muted-foreground/40 bg-muted-foreground/30",
};

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
  const assigneePubkeys = React.useMemo(
    () => [...new Set(tree.nodes.flatMap((n) => n.assignees ?? []))],
    [tree],
  );
  const usersQuery = useUsersBatchQuery(assigneePubkeys);
  const nameOf = React.useCallback(
    (pubkey: string) =>
      usersQuery.data?.profiles[pubkey]?.displayName ?? "Unknown member",
    [usersQuery.data],
  );

  const apply = React.useCallback(
    (op: GoalOp) => applyMutation.mutateAsync(op),
    [applyMutation],
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

type Apply = (op: GoalOp) => Promise<unknown>;

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

function GoalEditor({
  initialNote,
  initialTitle,
  onCancel,
  onSave,
  submitLabel = "Save",
  titleLabel,
}: {
  initialNote: string;
  initialTitle: string;
  onCancel?: () => void;
  onSave: (title: string, note: string | null) => Promise<unknown>;
  submitLabel?: string;
  titleLabel: string;
}) {
  const [title, setTitle] = React.useState(initialTitle);
  const [note, setNote] = React.useState(initialNote);
  const [saving, setSaving] = React.useState(false);
  const titleId = React.useId();
  const noteId = React.useId();
  const error = title === initialTitle ? null : goalTitleError(title);

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    if (goalTitleError(title) || saving) return;
    setSaving(true);
    try {
      await onSave(title.trim(), note.trim().length > 0 ? note : null);
      if (!onCancel) {
        setTitle("");
        setNote("");
      }
    } finally {
      setSaving(false);
    }
  };

  return (
    <form className="space-y-2" data-testid="goal-editor" onSubmit={submit}>
      <label
        className="block text-xs font-medium text-muted-foreground"
        htmlFor={titleId}
      >
        {titleLabel}
      </label>
      <Input
        id={titleId}
        autoFocus
        className="mt-1"
        data-testid="goal-editor-title"
        onChange={(event) =>
          setTitle(event.target.value.replace(/[\r\n]/g, " "))
        }
        value={title}
      />
      <label
        className="block text-xs font-medium text-muted-foreground"
        htmlFor={noteId}
      >
        Key information (optional)
      </label>
      <Textarea
        id={noteId}
        className="mt-1 min-h-16"
        data-testid="goal-editor-note"
        onChange={(event) => setNote(event.target.value)}
        value={note}
      />
      {error ? <p className="text-xs text-destructive">{error}</p> : null}
      <div className="flex justify-end gap-2">
        {onCancel ? (
          <Button onClick={onCancel} size="sm" type="button" variant="ghost">
            Cancel
          </Button>
        ) : null}
        <Button
          data-testid="goal-editor-save"
          disabled={saving || goalTitleError(title) !== null}
          size="sm"
          type="submit"
        >
          {saving ? "Saving…" : submitLabel}
        </Button>
      </div>
    </form>
  );
}

function AddGoalInline({
  apply,
  label,
  parentId,
}: {
  apply: Apply;
  label: string;
  parentId: string;
}) {
  const [open, setOpen] = React.useState(false);
  if (!open) {
    return (
      <Button
        className="text-muted-foreground"
        data-testid={`goal-add-${parentId}`}
        onClick={() => setOpen(true)}
        size="sm"
        type="button"
        variant="ghost"
      >
        <Plus className="mr-1 h-3.5 w-3.5" />
        {label}
      </Button>
    );
  }
  return (
    <div className="rounded-xl border border-border/60 p-3">
      <GoalEditor
        initialNote=""
        initialTitle=""
        onCancel={() => setOpen(false)}
        onSave={async (title, note) => {
          await apply({
            op: "add",
            id: newGoalId(),
            parent: parentId,
            title,
            note,
          });
          setOpen(false);
        }}
        submitLabel="Add"
        titleLabel="New goal"
      />
    </div>
  );
}

function GoalRow({
  apply,
  layer,
  nameOf,
  node,
  onOpenThread,
  tree,
}: {
  apply: Apply;
  layer: number;
  nameOf: (pubkey: string) => string;
  node: GoalNode;
  onOpenThread?: (threadRootId: string) => void;
  tree: GoalTree;
}) {
  const children = goalChildren(tree, node.id);
  const [expanded, setExpanded] = React.useState(layer < 3);
  const [editing, setEditing] = React.useState(false);
  const [confirmingRemove, setConfirmingRemove] = React.useState(false);
  const { done, total } = goalProgress(tree, node.id);
  const status = node.status;
  const nextStatus =
    STATUS_ORDER[(STATUS_ORDER.indexOf(status) + 1) % STATUS_ORDER.length];
  const subtreeSize = goalSubtreeIds(tree, node.id).size - 1;

  return (
    <div data-testid={`goal-row-${node.id}`}>
      <div className="group flex items-start gap-1.5 rounded-lg px-1 py-1 hover:bg-muted/40">
        <button
          aria-label={expanded ? "Collapse" : "Expand"}
          className="mt-0.5 flex h-4 w-4 shrink-0 items-center justify-center text-muted-foreground disabled:opacity-0"
          disabled={children.length === 0}
          onClick={() => setExpanded((value) => !value)}
          type="button"
        >
          {expanded ? (
            <ChevronDown className="h-3.5 w-3.5" />
          ) : (
            <ChevronRight className="h-3.5 w-3.5" />
          )}
        </button>
        <button
          aria-label={`Status: ${GOAL_STATUS_LABEL[status]}. Change to ${GOAL_STATUS_LABEL[nextStatus]}`}
          className={`mt-1 h-3 w-3 shrink-0 rounded-full border-2 ${STATUS_DOT[status]}`}
          data-testid={`goal-status-${node.id}`}
          onClick={() =>
            void apply({ op: "update", id: node.id, status: nextStatus })
          }
          title={GOAL_STATUS_LABEL[status]}
          type="button"
        />
        <div className="min-w-0 flex-1">
          <p
            className={`break-words text-sm ${status === "done" || status === "dropped" ? "text-muted-foreground line-through" : "text-foreground"}`}
          >
            <span className="mr-1 text-xs text-muted-foreground">L{layer}</span>
            {node.title}
          </p>
          <div className="flex flex-wrap items-center gap-x-2 text-xs text-muted-foreground">
            {total > 0 ? (
              <span>
                {done}/{total}
              </span>
            ) : null}
            {(node.assignees ?? []).map((pubkey) => (
              <span key={pubkey}>@{nameOf(pubkey)}</span>
            ))}
            {(node.threads ?? []).map((thread) => (
              <button
                className="inline-flex items-center gap-0.5 hover:text-foreground"
                data-testid={`goal-thread-${thread}`}
                disabled={!onOpenThread}
                key={thread}
                onClick={() => onOpenThread?.(thread)}
                type="button"
              >
                <MessagesSquare className="h-3 w-3" />
                Thread
              </button>
            ))}
          </div>
          {node.note && expanded ? (
            <p className="mt-0.5 whitespace-pre-wrap break-words text-xs text-muted-foreground">
              {node.note}
            </p>
          ) : null}
        </div>
        <div className="flex shrink-0 opacity-0 transition-opacity group-focus-within:opacity-100 group-hover:opacity-100">
          <Button
            aria-label="Edit goal"
            onClick={() => setEditing(true)}
            size="icon"
            type="button"
            variant="ghost"
          >
            <Pencil className="h-3.5 w-3.5" />
          </Button>
          <Button
            aria-label="Remove goal"
            data-testid={`goal-remove-${node.id}`}
            onClick={() => setConfirmingRemove(true)}
            size="icon"
            type="button"
            variant="ghost"
          >
            <Trash2 className="h-3.5 w-3.5" />
          </Button>
        </div>
      </div>

      {confirmingRemove ? (
        <div className="ml-6 flex items-center gap-2 rounded-lg bg-destructive/10 px-2 py-1.5 text-xs">
          <span className="flex-1 text-destructive">
            {subtreeSize > 0
              ? `Remove this goal and its ${subtreeSize} sub-goals? History keeps them.`
              : "Remove this goal? History keeps it."}
          </span>
          <Button
            onClick={() => setConfirmingRemove(false)}
            size="sm"
            type="button"
            variant="ghost"
          >
            Cancel
          </Button>
          <Button
            data-testid={`goal-remove-confirm-${node.id}`}
            onClick={() =>
              void apply({
                op: "remove",
                id: node.id,
                recursive: subtreeSize > 0,
              })
            }
            size="sm"
            type="button"
            variant="destructive"
          >
            Remove
          </Button>
        </div>
      ) : null}

      {editing ? (
        <div className="ml-6 rounded-xl border border-border/60 p-3">
          <GoalEditor
            initialNote={node.note ?? ""}
            initialTitle={node.title}
            onCancel={() => setEditing(false)}
            onSave={async (title, note) => {
              await apply({
                op: "update",
                id: node.id,
                title,
                note: note ?? "",
              });
              setEditing(false);
            }}
            titleLabel="Goal"
          />
        </div>
      ) : null}

      {expanded ? (
        <div className="ml-4 border-l border-border/50 pl-2">
          {children.map((child) => (
            <GoalRow
              apply={apply}
              key={child.id}
              layer={layer + 1}
              nameOf={nameOf}
              node={child}
              onOpenThread={onOpenThread}
              tree={tree}
            />
          ))}
          <AddGoalInline
            apply={apply}
            label={`Add a layer ${layer + 1} goal`}
            parentId={node.id}
          />
        </div>
      ) : null}
    </div>
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
