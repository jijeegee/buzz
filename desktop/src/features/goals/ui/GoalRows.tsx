import * as React from "react";
import {
  ChevronDown,
  ChevronRight,
  MessagesSquare,
  Pencil,
  Plus,
  Trash2,
} from "lucide-react";

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
  goalSubtreeIds,
  goalTitleError,
} from "../goalTree";

/**
 * Goal tree building blocks shared by the channel goals panel and the thread
 * goal panel: editor, inline "add a goal", status toggle, and outline row.
 */

const STATUS_ORDER: GoalStatus[] = ["open", "in_progress", "done", "dropped"];

const STATUS_DOT: Record<GoalStatus, string> = {
  open: "border-muted-foreground/60",
  in_progress: "border-primary bg-primary/30",
  done: "border-emerald-500 bg-emerald-500",
  dropped: "border-muted-foreground/40 bg-muted-foreground/30",
};

/** One edit, or several applied as a single revision. */
export type Apply = (op: GoalOp | GoalOp[]) => Promise<unknown>;

/**
 * Run a goal edit from a click handler. The failure is already shown by the
 * panel (from the mutation's error state); this only keeps the rejected
 * promise from going unhandled.
 */
export function runGoalEdit(edit: Promise<unknown>): void {
  edit.catch((error: unknown) => {
    console.warn("[goals] edit failed", error);
  });
}

/** The circle that cycles a goal through its statuses. */
export function GoalStatusButton({
  apply,
  className = "mt-1",
  node,
}: {
  apply: Apply;
  className?: string;
  node: GoalNode;
}) {
  const status = node.status;
  const nextStatus =
    STATUS_ORDER[(STATUS_ORDER.indexOf(status) + 1) % STATUS_ORDER.length];
  return (
    <button
      aria-label={`Status: ${GOAL_STATUS_LABEL[status]}. Change to ${GOAL_STATUS_LABEL[nextStatus]}`}
      className={`${className} h-3 w-3 shrink-0 rounded-full border-2 ${STATUS_DOT[status]}`}
      data-testid={`goal-status-${node.id}`}
      onClick={() =>
        runGoalEdit(apply({ op: "update", id: node.id, status: nextStatus }))
      }
      title={GOAL_STATUS_LABEL[status]}
      type="button"
    />
  );
}

export function GoalEditor({
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
    } catch (error) {
      // Keep the draft open; the panel shows the failure.
      console.warn("[goals] save failed", error);
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

export function AddGoalInline({
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

export function GoalRow({
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
        <GoalStatusButton apply={apply} node={node} />
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
              runGoalEdit(
                apply({
                  op: "remove",
                  id: node.id,
                  recursive: subtreeSize > 0,
                }),
              )
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
