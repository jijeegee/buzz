import * as React from "react";
import {
  ChevronDown,
  ChevronRight,
  History,
  Pencil,
  Target,
} from "lucide-react";

import { useUsersBatchQuery } from "@/features/profile/hooks";
import type { GoalEdit, GoalNode, GoalView } from "@/shared/api/tauriGoals";
import { newGoalId } from "@/shared/api/tauriGoals";
import { Button } from "@/shared/ui/button";
import {
  type GoalLinkTarget,
  goalChildren,
  goalListTops,
  goalPath,
  goalRoot,
  mergeGoalTrees,
} from "../goalTree";
import {
  useApplyGoalOpMutation,
  useGoalAssigneeNames,
  useGoalHistoryQuery,
  useGoalLinkKinds,
  useGoalListQuery,
  useGoalTreeQuery,
} from "../hooks";
import {
  AddGoalInline,
  type Apply,
  GoalEditor,
  GoalRow,
  runGoalEdit,
} from "./GoalRows";
import { formatLocale, type MessageKey, useT } from "@/shared/i18n";

/** Above this many active goals under layer 1, the panel says so. */
const TOP_GOALS_NOTICE = 7;

type GoalsPanelProps = {
  channelId: string;
  onOpenLink?: (target: GoalLinkTarget) => void;
};

/**
 * The goals of a channel or DM: layer 1 card, the active outline, the quiet
 * and done lists (read when opened), and the edit history.
 */
export function GoalsPanel({ channelId, onOpenLink }: GoalsPanelProps) {
  const t = useT();
  const goalQuery = useGoalTreeQuery(channelId);
  const applyMutation = useApplyGoalOpMutation(channelId);
  const [showHistory, setShowHistory] = React.useState(false);
  const tree = goalQuery.data?.tree ?? { nodes: [] };
  const counts = goalQuery.data?.counts;
  const root = goalRoot(tree);
  const nameOf = useGoalAssigneeNames(goalQuery.data?.tree);
  const linkKinds = useGoalLinkKinds(channelId, goalQuery.data?.tree);

  const applyGoalOp = applyMutation.mutateAsync;
  const apply = React.useCallback<Apply>(
    (op) => applyGoalOp(op),
    [applyGoalOp],
  );

  if (goalQuery.isLoading) {
    return (
      <p className="pt-4 text-sm text-muted-foreground">
        {t("goals.panel.loading")}
      </p>
    );
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
      {root && root.open_children > TOP_GOALS_NOTICE ? (
        <p
          className="rounded-xl bg-muted/40 px-3 py-2 text-xs text-muted-foreground"
          data-testid="goals-top-notice"
        >
          {t("goals.panel.topGoals", { count: root.open_children })}
        </p>
      ) : null}

      {root ? (
        <RootCard apply={apply} root={root} />
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
              linkKinds={linkKinds}
              nameOf={nameOf}
              node={child}
              onOpenLink={onOpenLink}
              tree={tree}
            />
          ))}
          <AddGoalInline
            apply={apply}
            label={t("goals.addLayerGoal", { layer: 2 })}
            parentId={root.id}
          />
        </div>
      ) : null}

      <GoalListSection
        apply={apply}
        channelId={channelId}
        count={counts?.inactive ?? 0}
        label={t("goals.section.quiet")}
        onOpenLink={onOpenLink}
        view="inactive"
      />
      <GoalListSection
        apply={apply}
        channelId={channelId}
        count={counts?.done ?? 0}
        label={t("goals.section.done")}
        onOpenLink={onOpenLink}
        view="done"
      />

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
          {showHistory
            ? t("goals.panel.hideHistory")
            : t("goals.panel.history")}
        </Button>
        {showHistory ? <GoalHistory channelId={channelId} /> : null}
      </div>
    </div>
  );
}

/** A list that folds away and reads its goals when opened. */
function GoalListSection({
  apply,
  channelId,
  count,
  label,
  onOpenLink,
  view,
}: {
  apply: Apply;
  channelId: string;
  count: number;
  label: string;
  onOpenLink?: (target: GoalLinkTarget) => void;
  view: Exclude<GoalView, "active">;
}) {
  const [open, setOpen] = React.useState(false);
  if (count === 0) return null;
  return (
    <section data-testid={`goals-section-${view}`}>
      <button
        aria-expanded={open}
        className="flex w-full items-center gap-1 rounded-lg px-1 py-1 text-left text-xs font-medium text-muted-foreground hover:bg-muted/40"
        data-testid={`goals-section-toggle-${view}`}
        onClick={() => setOpen((value) => !value)}
        type="button"
      >
        {open ? (
          <ChevronDown className="h-3.5 w-3.5" />
        ) : (
          <ChevronRight className="h-3.5 w-3.5" />
        )}
        <span className="flex-1">{label}</span>
        <span>{count}</span>
      </button>
      {open ? (
        <GoalListRows
          apply={apply}
          channelId={channelId}
          onOpenLink={onOpenLink}
          view={view}
        />
      ) : null}
    </section>
  );
}

function GoalListRows({
  apply,
  channelId,
  onOpenLink,
  view,
}: {
  apply: Apply;
  channelId: string;
  onOpenLink?: (target: GoalLinkTarget) => void;
  view: Exclude<GoalView, "active">;
}) {
  const t = useT();
  const listQuery = useGoalListQuery(channelId, view);
  const tree = React.useMemo(
    () =>
      mergeGoalTrees(
        (listQuery.data?.pages ?? []).map((page) => ({ nodes: page.rows })),
      ),
    [listQuery.data],
  );
  const nameOf = useGoalAssigneeNames(tree);
  const linkKinds = useGoalLinkKinds(channelId, tree);

  if (listQuery.isLoading) {
    return (
      <p className="px-1 py-1 text-xs text-muted-foreground">
        {t("goals.panel.loading")}
      </p>
    );
  }
  if (listQuery.error instanceof Error) {
    return (
      <p className="px-1 py-1 text-xs text-destructive">
        {listQuery.error.message}
      </p>
    );
  }
  return (
    <div className="space-y-1 pt-1">
      {view === "inactive" ? <DropQuietGoals apply={apply} /> : null}
      {goalListTops(tree).map((node) => {
        const path = goalPath(tree, node.id);
        const above = path
          .slice(1, -1)
          .map((row) => row.title)
          .join(" › ");
        return (
          <div key={node.id}>
            {above ? (
              <p className="truncate pl-6 text-xs text-muted-foreground">
                {above}
              </p>
            ) : null}
            <GoalRow
              apply={apply}
              layer={path.length}
              linkKinds={linkKinds}
              nameOf={nameOf}
              node={node}
              onOpenLink={onOpenLink}
              tree={tree}
            />
          </div>
        );
      })}
      {listQuery.hasNextPage ? (
        <Button
          disabled={listQuery.isFetchingNextPage}
          onClick={() => void listQuery.fetchNextPage()}
          size="sm"
          type="button"
          variant="ghost"
        >
          {t("goals.section.showMore")}
        </Button>
      ) : null}
    </div>
  );
}

function DropQuietGoals({ apply }: { apply: Apply }) {
  const t = useT();
  const [confirming, setConfirming] = React.useState(false);
  if (!confirming) {
    return (
      <Button
        className="text-muted-foreground"
        data-testid="goals-drop-quiet"
        onClick={() => setConfirming(true)}
        size="sm"
        type="button"
        variant="ghost"
      >
        {t("goals.section.dropAll")}
      </Button>
    );
  }
  return (
    <div className="flex items-center gap-2 rounded-lg bg-destructive/10 px-2 py-1.5 text-xs">
      <span className="flex-1 text-destructive">
        {t("goals.section.dropAllConfirm")}
      </span>
      <Button
        onClick={() => setConfirming(false)}
        size="sm"
        type="button"
        variant="ghost"
      >
        {t("common.cancel")}
      </Button>
      <Button
        data-testid="goals-drop-quiet-confirm"
        onClick={() => {
          setConfirming(false);
          runGoalEdit(apply({ op: "drop_inactive" }));
        }}
        size="sm"
        type="button"
        variant="destructive"
      >
        {t("goals.section.dropAll")}
      </Button>
    </div>
  );
}

function RootCard({ apply, root }: { apply: Apply; root: GoalNode }) {
  const t = useT();
  const [editing, setEditing] = React.useState(false);
  const { done, total } = root.progress;
  const percent = total > 0 ? Math.round((done / total) * 100) : 0;

  if (editing) {
    return (
      <GoalEditor
        initialNote={root.note}
        initialTitle={root.title}
        onCancel={() => setEditing(false)}
        onSave={async (title, note) => {
          await apply({ op: "set_root", id: root.id, title, note });
          setEditing(false);
        }}
        titleLabel={t("goals.rootCard.titleLabel")}
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
            {t("goals.rootCard.heading")}
          </p>
          <p className="mt-1 break-words text-sm font-semibold text-foreground">
            {root.title}
          </p>
        </div>
        <Button
          aria-label={t("goals.rootCard.edit")}
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
            {t("goals.rootCard.progress", { done, total })}
          </p>
        </div>
      ) : null}
    </section>
  );
}

function SetRootForm({ apply }: { apply: Apply }) {
  const t = useT();
  return (
    <section className="space-y-2" data-testid="goals-empty">
      <p className="text-sm text-muted-foreground">
        {t("goals.setRoot.intro")}
      </p>
      <GoalEditor
        initialNote=""
        initialTitle=""
        onSave={(title, note) =>
          apply({ op: "set_root", id: newGoalId(), title, note })
        }
        submitLabel={t("goals.setRoot.submit")}
        titleLabel={t("goals.rootCard.titleLabel")}
      />
    </section>
  );
}

const EDIT_LABEL: Record<GoalEdit["ops"][number]["op"], MessageKey> = {
  set_root: "goals.history.setRoot",
  add: "goals.history.add",
  update: "goals.history.update",
  move: "goals.history.move",
  remove: "goals.history.remove",
  link: "goals.history.link",
  unlink: "goals.history.unlink",
  drop_inactive: "goals.history.dropQuiet",
};

function GoalHistory({ channelId }: { channelId: string }) {
  const historyQuery = useGoalHistoryQuery(channelId, true);
  const edits = historyQuery.data ?? [];
  const authors = React.useMemo(
    () => [...new Set(edits.map((edit) => edit.author))],
    [edits],
  );
  const usersQuery = useUsersBatchQuery(authors);
  const t = useT();

  if (historyQuery.isLoading) {
    return (
      <p className="mt-2 text-xs text-muted-foreground">
        {t("goals.history.loading")}
      </p>
    );
  }
  return (
    <ul className="mt-2 space-y-1" data-testid="goals-history">
      {edits.map((edit) => (
        <li
          className="rounded-lg px-2 py-1 text-xs text-muted-foreground hover:bg-muted/40"
          key={edit.eventId}
        >
          {t("goals.history.edit", {
            time: new Date(edit.createdAt * 1000).toLocaleString(
              formatLocale(),
            ),
            author:
              usersQuery.data?.profiles[edit.author]?.displayName ??
              t("goals.unknownMember"),
            edits: [
              ...new Set(edit.ops.map((op) => t(EDIT_LABEL[op.op]))),
            ].join(", "),
          })}
        </li>
      ))}
    </ul>
  );
}
