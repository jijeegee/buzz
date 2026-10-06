import { invokeTauri } from "@/shared/api/tauri";

export type GoalStatus = "open" | "in_progress" | "done" | "dropped";

export type GoalNode = {
  id: string;
  parent: string | null;
  title: string;
  note?: string;
  status: GoalStatus;
  assignees?: string[];
  threads?: string[];
  order: number;
  updated_by?: string;
  updated_at?: number;
};

export type GoalTree = { v: number; nodes: GoalNode[] };

export type GoalTreeHead = {
  revision: string | null;
  updatedAt: number | null;
  author: string | null;
  tree: GoalTree;
};

/** One edit; mirrors `buzz_core::goal_tree::GoalOp`. */
export type GoalOp =
  | { op: "set_root"; id: string; title: string; note?: string | null }
  | {
      op: "add";
      id: string;
      parent: string;
      title: string;
      note?: string | null;
      assignees?: string[];
    }
  | {
      op: "update";
      id: string;
      title?: string | null;
      note?: string | null;
      status?: GoalStatus | null;
      add_assignees?: string[];
      remove_assignees?: string[];
    }
  | { op: "move"; id: string; parent: string; order?: number | null }
  | { op: "remove"; id: string; recursive: boolean }
  | { op: "link"; id: string; thread: string }
  | { op: "unlink"; thread: string };

export type GoalTreeRevision = {
  revision: string;
  createdAt: number;
  author: string;
  goals: number | null;
};

type RawHead = {
  revision: string | null;
  updated_at: number | null;
  author: string | null;
  tree: GoalTree;
};

export async function getGoalTree(channelId: string): Promise<GoalTreeHead> {
  const raw = await invokeTauri<RawHead>("get_goal_tree", { channelId });
  return {
    revision: raw.revision ?? null,
    updatedAt: raw.updated_at ?? null,
    author: raw.author ?? null,
    tree: raw.tree,
  };
}

export async function applyGoalOp(
  channelId: string,
  op: GoalOp,
): Promise<{ eventId: string; tree: GoalTree }> {
  const raw = await invokeTauri<{ event_id: string; tree: GoalTree }>(
    "apply_goal_op",
    { channelId, op },
  );
  return { eventId: raw.event_id, tree: raw.tree };
}

export async function getGoalTreeHistory(
  channelId: string,
): Promise<GoalTreeRevision[]> {
  const raw = await invokeTauri<{
    revisions: {
      revision: string;
      created_at: number;
      author: string;
      goals: number | null;
    }[];
  }>("get_goal_tree_history", { channelId, limit: 30 });
  return raw.revisions.map((r) => ({
    revision: r.revision,
    createdAt: r.created_at,
    author: r.author,
    goals: r.goals,
  }));
}

export async function restoreGoalTree(
  channelId: string,
  revision: string,
): Promise<void> {
  await invokeTauri("restore_goal_tree", { channelId, revision });
}

/** A fresh node id in the same shape as `buzz_core::goal_tree::new_node_id`. */
export function newGoalId(): string {
  const bytes = new Uint8Array(8);
  crypto.getRandomValues(bytes);
  return `g_${Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("")}`;
}
