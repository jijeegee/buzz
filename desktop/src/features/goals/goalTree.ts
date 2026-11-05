import type { GoalNode, GoalStatus, GoalTree } from "@/shared/api/tauriGoals";
import { type MessageKey, t } from "@/shared/i18n";

export type OutlineRow = { layer: number; node: GoalNode };

export function goalRoot(tree: GoalTree): GoalNode | null {
  return tree.nodes.find((node) => node.parent === null) ?? null;
}

/** Plan goals first, thread goals below them. */
export function goalChildren(tree: GoalTree, id: string): GoalNode[] {
  return tree.nodes
    .filter((node) => node.parent === id)
    .sort(
      (a, b) =>
        Number(a.is_thread) - Number(b.is_thread) ||
        a.order - b.order ||
        a.id.localeCompare(b.id),
    );
}

/** Depth-first display order with layers (root = layer 1). */
export function goalOutline(tree: GoalTree): OutlineRow[] {
  const rows: OutlineRow[] = [];
  const root = goalRoot(tree);
  if (!root) return rows;
  const walk = (node: GoalNode, layer: number) => {
    rows.push({ layer, node });
    if (layer >= 128) return;
    for (const child of goalChildren(tree, node.id)) walk(child, layer + 1);
  };
  walk(root, 1);
  return rows;
}

/** Nodes from the root down to `id`, inclusive. */
export function goalPath(tree: GoalTree, id: string): GoalNode[] {
  const byId = new Map(tree.nodes.map((node) => [node.id, node]));
  const path: GoalNode[] = [];
  let cursor = byId.get(id);
  while (cursor && path.length <= 128) {
    path.unshift(cursor);
    cursor = cursor.parent ? byId.get(cursor.parent) : undefined;
  }
  return path;
}

/** A goal with no thread link, here or above; it shows unfolded. */
export function isPlanGoal(tree: GoalTree, id: string): boolean {
  return !goalPath(tree, id).some((node) => node.is_thread);
}

/**
 * Several reads as one tree. A goal in more than one shows the last, and
 * stays selected if any read selected it: a later page brings it back as the
 * parent of what it selected.
 */
export function mergeGoalTrees(trees: GoalTree[]): GoalTree {
  const byId = new Map<string, GoalNode>();
  for (const node of trees.flatMap((tree) => tree.nodes)) {
    const matched = node.matched || byId.get(node.id)?.matched === true;
    byId.set(node.id, { ...node, matched });
  }
  return { nodes: [...byId.values()] };
}

/**
 * The selected goals a list shows at its top level, in order. Layer 1 keeps
 * to its card: a row would offer to remove the whole room.
 */
export function goalListTops(tree: GoalTree): GoalNode[] {
  return tree.nodes
    .filter(
      (node) =>
        node.matched &&
        node.parent !== null &&
        !goalPath(tree, node.id)
          .slice(1, -1)
          .some((above) => above.matched),
    )
    .sort((a, b) => a.order - b.order || a.id.localeCompare(b.id));
}

/**
 * The id a thread is linked to goals by: the root of its reply chain, the
 * same id mobile and agents use. A nested thread head links its chain root.
 */
export function threadGoalRootId(head: {
  id: string;
  rootId?: string | null;
}): string {
  return head.rootId ?? head.id;
}

export function goalForThread(
  tree: GoalTree,
  threadRootId: string,
): GoalNode | null {
  const thread = threadRootId.toLowerCase();
  return tree.nodes.find((node) => node.links.includes(thread)) ?? null;
}

/** Ids of `id` and everything below it. */
export function goalSubtreeIds(tree: GoalTree, id: string): Set<string> {
  const ids = new Set<string>([id]);
  const stack = [id];
  while (stack.length > 0) {
    const current = stack.pop() as string;
    for (const child of goalChildren(tree, current)) {
      ids.add(child.id);
      stack.push(child.id);
    }
  }
  return ids;
}

/** Finishing this goal deletes the tasks below it (the relay's rule). */
export function finishDeletesTasks(node: GoalNode, next: GoalStatus): boolean {
  const running = node.status === "open" || node.status === "in_progress";
  const finishing = next === "done" || next === "dropped";
  return (
    running &&
    finishing &&
    node.is_thread &&
    (node.progress.total > 0 || node.open_children > 0)
  );
}

/** Message keys of the status names; translate where shown. */
export const GOAL_STATUS_LABEL: Record<GoalNode["status"], MessageKey> = {
  open: "goals.status.open",
  in_progress: "goals.status.inProgress",
  done: "goals.status.done",
  dropped: "goals.status.dropped",
};

export const GOAL_TITLE_MAX_CHARS = 200;

/** Client-side mirror of the relay's title rule (single line, ≤200 chars). */
export function goalTitleError(title: string): string | null {
  if (title.trim().length === 0) return t("goals.titleError.empty");
  if (/[\r\n]/.test(title)) return t("goals.titleError.oneLine");
  if ([...title].length > GOAL_TITLE_MAX_CHARS)
    return t("goals.titleError.tooLong", { max: GOAL_TITLE_MAX_CHARS });
  return null;
}

/**
 * How a goal shows in a thread linked to `goalId`: the linked goal and its
 * sub-goals are editable, the path above it is shown but read-only, and every
 * other goal is read-only and faded. Nested threads link their chain root
 * (see `threadGoalRootId`), so they share the outer thread's scope.
 */
export type GoalAccess = "focus" | "edit" | "path" | "muted";

export function threadGoalAccess(
  tree: GoalTree,
  goalId: string,
): (id: string) => GoalAccess {
  const editable = goalSubtreeIds(tree, goalId);
  const path = new Set(goalPath(tree, goalId).map((node) => node.id));
  return (id) => {
    if (id === goalId) return "focus";
    if (editable.has(id)) return "edit";
    if (path.has(id)) return "path";
    return "muted";
  };
}

/**
 * Where a goal may be linked from a message's menu. In a thread the message
 * may link only the thread's goal and its sub-goals, or layer 2 and below
 * while the thread has none, as may a main-timeline message. New goals may
 * be added under any goal whose child would be linkable.
 */
export type GoalLinkScope = {
  canAddUnder: (id: string) => boolean;
  canLink: (id: string, layer: number) => boolean;
};

export function messageGoalLinkScope(
  tree: GoalTree,
  threadRootId: string | null,
): GoalLinkScope {
  const threadGoal = threadRootId ? goalForThread(tree, threadRootId) : null;
  if (threadGoal) {
    const editable = goalSubtreeIds(tree, threadGoal.id);
    return {
      canAddUnder: (id) => editable.has(id),
      canLink: (id) => editable.has(id),
    };
  }
  return {
    canAddUnder: () => true,
    canLink: (_id, layer) => layer >= 2,
  };
}

/**
 * What a goal's linked id is. Links share one list: a main-timeline message
 * with replies is a thread, one without is a message, and a reply is a
 * message inside its thread. `missing` covers deleted or unknown events.
 */
export type GoalLinkInfo =
  | { kind: "thread" | "message" | "missing" }
  | { kind: "reply"; threadRootId: string };

export type GoalLinkTarget =
  | { kind: "thread"; threadRootId: string }
  | { kind: "message"; messageId: string; threadRootId: string | null };

/**
 * Label and destination of a linked id. Unknown links (still loading) act
 * as threads, as they did before message links; missing ones go nowhere.
 */
export function goalLinkView(
  id: string,
  info: GoalLinkInfo | undefined,
): { label: "Thread" | "Message"; target: GoalLinkTarget | null } {
  switch (info?.kind) {
    case "message":
      return {
        label: "Message",
        target: { kind: "message", messageId: id, threadRootId: null },
      };
    case "reply":
      return {
        label: "Message",
        target: {
          kind: "message",
          messageId: id,
          threadRootId: info.threadRootId,
        },
      };
    case "missing":
      return { label: "Message", target: null };
    default:
      return { label: "Thread", target: { kind: "thread", threadRootId: id } };
  }
}
