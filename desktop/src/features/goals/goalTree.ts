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
