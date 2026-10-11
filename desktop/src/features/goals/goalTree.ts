import type { GoalNode, GoalTree } from "@/shared/api/tauriGoals";

export type OutlineRow = { layer: number; node: GoalNode };

export function goalRoot(tree: GoalTree): GoalNode | null {
  return tree.nodes.find((node) => node.parent === null) ?? null;
}

export function goalChildren(tree: GoalTree, id: string): GoalNode[] {
  return tree.nodes
    .filter((node) => node.parent === id)
    .sort((a, b) => a.order - b.order || a.id.localeCompare(b.id));
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

/** `done`/`total` over all descendants of `id`. */
export function goalProgress(
  tree: GoalTree,
  id: string,
): { done: number; total: number } {
  let done = 0;
  let total = 0;
  const stack = goalChildren(tree, id);
  while (stack.length > 0) {
    const node = stack.pop() as GoalNode;
    total += 1;
    if (node.status === "done") done += 1;
    stack.push(...goalChildren(tree, node.id));
  }
  return { done, total };
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
  return tree.nodes.find((node) => node.threads?.includes(thread)) ?? null;
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

export const GOAL_STATUS_LABEL: Record<GoalNode["status"], string> = {
  open: "To do",
  in_progress: "In progress",
  done: "Done",
  dropped: "Dropped",
};

export const GOAL_TITLE_MAX_CHARS = 200;

/** Client-side mirror of the relay's title rule (single line, ≤200 chars). */
export function goalTitleError(title: string): string | null {
  if (title.trim().length === 0) return "Write the goal in one sentence.";
  if (/[\r\n]/.test(title)) return "Keep the goal on one line.";
  if ([...title].length > GOAL_TITLE_MAX_CHARS)
    return `Keep the goal under ${GOAL_TITLE_MAX_CHARS} characters.`;
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
 * may link only the thread's goal and its sub-goals, or any goal while the
 * thread has none (the thread's own picker rule); on the main timeline it
 * may link layer 2 and below. New goals may be added under any goal whose
 * child would be linkable.
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
    canLink: (_id, layer) => threadRootId !== null || layer >= 2,
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
