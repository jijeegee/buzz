/**
 * E2E mock of the goal tree and layer 0 goal Tauri commands. Mirrors the
 * relevant `buzz_core::goal_tree::GoalTree::apply` rules (including "linking
 * a thread starts an open goal") closely enough for UI tests; validation is
 * left to the real backend.
 */
import type { GoalNode, GoalOp, GoalTree } from "@/shared/api/tauriGoals";

const trees = new Map<string, GoalTree>();
const revisions = new Map<string, number>();
const layer0 = new Map<string, unknown>();

const EDITOR = "e2e";

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
        nodes.push({
          id: op.id,
          parent: null,
          title: op.title,
          note: op.note ?? undefined,
          status: "open",
          order: 0,
        });
      }
      break;
    }
    case "add": {
      find(op.parent);
      const order = nodes.filter((n) => n.parent === op.parent).length;
      nodes.push({
        id: op.id,
        parent: op.parent,
        title: op.title,
        note: op.note ?? undefined,
        status: "open",
        assignees: op.assignees,
        order,
      });
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
      for (const n of nodes) n.threads = n.threads?.filter((t) => t !== thread);
      const node = find(op.id);
      node.threads = [...(node.threads ?? []), thread];
      if (node.status === "open") node.status = "in_progress";
      touch(node);
      break;
    }
    case "unlink": {
      const thread = op.thread.toLowerCase();
      for (const n of nodes) n.threads = n.threads?.filter((t) => t !== thread);
      break;
    }
  }
  return { ...tree, nodes };
}

function head(channelId: string) {
  const tree = trees.get(channelId);
  return tree
    ? {
        revision: `rev-${revisions.get(channelId) ?? 0}`,
        updated_at: Math.floor(Date.now() / 1000),
        author: EDITOR,
        tree,
      }
    : {
        revision: null,
        updated_at: null,
        author: null,
        tree: { v: 1, nodes: [] },
      };
}

function write(channelId: string, ops: GoalOp[]) {
  let tree = trees.get(channelId) ?? { v: 1, nodes: [] };
  for (const op of ops) tree = applyOp(tree, op);
  trees.set(channelId, tree);
  const revision = (revisions.get(channelId) ?? 0) + 1;
  revisions.set(channelId, revision);
  return { event_id: `rev-${revision}`, tree };
}

/** Handle a goal command, or return `undefined` when it is not one. */
export function handleGoalsMockCommand(
  command: string,
  payload: unknown,
): { value: unknown } | undefined {
  const args = (payload ?? {}) as Record<string, unknown>;
  const channelId = String(args.channelId ?? "");
  switch (command) {
    case "get_goal_tree":
      return { value: head(channelId) };
    case "apply_goal_op":
      return { value: write(channelId, [args.op as GoalOp]) };
    case "apply_goal_ops":
      return { value: write(channelId, args.ops as GoalOp[]) };
    case "get_goal_tree_history":
      return { value: { revisions: [] } };
    case "restore_goal_tree":
      return { value: head(channelId) };
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

/** Forget every mocked goal tree (between tests). */
export function resetGoalsMock(): void {
  trees.clear();
  revisions.clear();
  layer0.clear();
}
