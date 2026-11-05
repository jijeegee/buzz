/**
 * E2E mock of the goal and layer 0 goal Tauri commands. Reads return rows the
 * way the relay does; edits mirror the relevant `kuzz_core` op rules (including
 * "linking a thread starts an open goal") closely enough for UI tests.
 * Validation and the deletion rule are left to the real relay.
 */
import type {
  GoalNode,
  GoalOp,
  GoalRead,
  GoalTree,
  GoalView,
} from "@/shared/api/tauriGoals";

const trees = new Map<string, GoalTree>();
const writes = new Map<string, number>();
const layer0 = new Map<string, unknown>();

const EDITOR = "e2e";

function newNode(
  fields: Pick<GoalNode, "id" | "parent" | "title" | "order"> &
    Partial<GoalNode>,
): GoalNode {
  return {
    note: "",
    status: "open",
    assignees: [],
    links: [],
    updated_by: EDITOR,
    updated_at: Math.floor(Date.now() / 1000),
    matched: false,
    inactive: false,
    is_thread: false,
    progress: { done: 0, total: 0 },
    open_children: 0,
    ...fields,
  };
}

function applyOp(tree: GoalTree, op: GoalOp): GoalTree {
  const nodes = tree.nodes.map((node) => ({ ...node }));
  const find = (id: string) => {
    const node = nodes.find((n) => n.id === id);
    if (!node) throw new Error(`goal ${id} not found`);
    return node;
  };
  const now = Math.floor(Date.now() / 1000);
  const touch = (node: GoalNode) => {
    node.updated_by = EDITOR;
    node.updated_at = now;
  };
  switch (op.op) {
    case "set_root": {
      const root = nodes.find((n) => n.parent === null);
      if (root) {
        root.title = op.title;
        root.note = op.note ?? root.note;
        touch(root);
      } else {
        nodes.push(
          newNode({
            id: op.id,
            parent: null,
            title: op.title,
            note: op.note ?? "",
            order: 0,
          }),
        );
      }
      break;
    }
    case "add": {
      find(op.parent);
      const order = nodes.filter((n) => n.parent === op.parent).length;
      nodes.push(
        newNode({
          id: op.id,
          parent: op.parent,
          title: op.title,
          note: op.note ?? "",
          assignees: op.assignees ?? [],
          order,
        }),
      );
      break;
    }
    case "update": {
      const node = find(op.id);
      if (op.title != null) node.title = op.title;
      if (op.note != null) node.note = op.note;
      if (op.status != null) node.status = op.status;
      touch(node);
      break;
    }
    case "move": {
      find(op.id).parent = op.parent;
      break;
    }
    case "remove": {
      const doomed = new Set([op.id]);
      let grew = true;
      while (grew) {
        grew = false;
        for (const n of nodes) {
          if (n.parent && doomed.has(n.parent) && !doomed.has(n.id)) {
            doomed.add(n.id);
            grew = true;
          }
        }
      }
      return { ...tree, nodes: nodes.filter((n) => !doomed.has(n.id)) };
    }
    case "link": {
      const thread = op.thread.toLowerCase();
      for (const n of nodes) n.links = n.links.filter((t) => t !== thread);
      const node = find(op.id);
      node.links = [...node.links, thread];
      node.is_thread = true;
      if (node.status === "open") node.status = "in_progress";
      touch(node);
      break;
    }
    case "unlink": {
      const thread = op.thread.toLowerCase();
      for (const n of nodes) {
        n.links = n.links.filter((t) => t !== thread);
        n.is_thread = n.links.length > 0;
      }
      break;
    }
    case "drop_inactive":
      break;
  }
  return { ...tree, nodes };
}

function viewOf(node: GoalNode): GoalView {
  return node.status === "done" || node.status === "dropped"
    ? "done"
    : "active";
}

/** The selected goals with the goals above them, the way the relay reads. */
function read(tree: GoalTree, selected: GoalNode[]): GoalRead {
  const byId = new Map(tree.nodes.map((node) => [node.id, node]));
  const chosen = new Set(selected.map((node) => node.id));
  const shown = new Set(chosen);
  for (const node of selected) {
    for (let up = node.parent; up; up = byId.get(up)?.parent ?? null) {
      shown.add(up);
    }
  }
  const belowOf = (id: string): GoalNode[] =>
    tree.nodes
      .filter((node) => node.parent === id)
      .flatMap((node) => [node, ...belowOf(node.id)]);
  return {
    counts: {
      active: tree.nodes.filter((n) => viewOf(n) === "active").length,
      inactive: 0,
      done: tree.nodes.filter((n) => viewOf(n) === "done").length,
    },
    rows: tree.nodes
      .filter((node) => shown.has(node.id))
      .map((node) => ({
        ...node,
        matched: chosen.has(node.id),
        progress: {
          done: belowOf(node.id).filter((n) => viewOf(n) === "done").length,
          total: belowOf(node.id).length,
        },
        open_children: tree.nodes.filter(
          (n) => n.parent === node.id && viewOf(n) === "active",
        ).length,
      })),
    threads: [],
    next: null,
  };
}

function treeOf(channelId: string): GoalTree {
  return trees.get(channelId) ?? { nodes: [] };
}

function readView(channelId: string, view: GoalView): GoalRead {
  const tree = treeOf(channelId);
  return read(
    tree,
    tree.nodes.filter((node) => viewOf(node) === view),
  );
}

function readThreads(channelId: string, threads: string[]): GoalRead {
  const tree = treeOf(channelId);
  const goalOf = (thread: string) =>
    tree.nodes.find((node) => node.links.includes(thread));
  return {
    ...read(
      tree,
      tree.nodes.filter((node) => threads.some((t) => goalOf(t) === node)),
    ),
    threads: threads.map((thread) => ({
      thread,
      goal: goalOf(thread)?.id ?? null,
    })),
  };
}

function write(channelId: string, ops: GoalOp[]) {
  let tree = treeOf(channelId);
  for (const op of ops) tree = applyOp(tree, op);
  trees.set(channelId, tree);
  const count = (writes.get(channelId) ?? 0) + 1;
  writes.set(channelId, count);
  return { event_id: `edit-${count}` };
}

/** Handle a goal command, or return `undefined` when it is not one. */
export function handleGoalsMockCommand(
  command: string,
  payload: unknown,
): { value: unknown } | undefined {
  const args = (payload ?? {}) as Record<string, unknown>;
  const channelId = String(args.channelId ?? "");
  switch (command) {
    case "get_goals":
      return { value: readView(channelId, args.view as GoalView) };
    case "get_thread_goals":
      return { value: readThreads(channelId, args.threads as string[]) };
    case "apply_goal_ops":
      return { value: write(channelId, args.ops as GoalOp[]) };
    case "get_goal_history":
      return { value: { edits: [] } };
    case "get_layer0_goal":
      return {
        value: layer0.get(String(args.agentPubkey ?? "self")) ?? {
          private: "",
          public: "",
          public_enabled: false,
        },
      };
    case "set_layer0_goal":
      layer0.set(String(args.agentPubkey ?? "self"), args.goal);
      return { value: null };
    case "get_public_goal":
      return { value: null };
    default:
      return undefined;
  }
}

/** Forget every mocked goal (between tests). */
export function resetGoalsMock(): void {
  trees.clear();
  writes.clear();
  layer0.clear();
}
