import { invokeTauri } from "@/shared/api/tauri";

export type GoalStatus = "open" | "in_progress" | "done" | "dropped";

/** One goal as the relay reads it, with what the relay computed. */
export type GoalNode = {
  id: string;
  parent: string | null;
  title: string;
  note: string;
  status: GoalStatus;
  order: number;
  assignees: string[];
  /** Linked thread and message ids. */
  links: string[];
  updated_by: string | null;
  updated_at: number;
  /** Selected by the read, as opposed to shown as a parent of one that was. */
  matched: boolean;
  /** Unfinished with no activity for the inactive window. */
  inactive: boolean;
  /** A linked thread has replies. */
  is_thread: boolean;
  /** Goals below that count; the relay decides which do. */
  progress: { done: number; total: number };
  /** Active goals directly below. */
  open_children: number;
};

export type GoalTree = { nodes: GoalNode[] };

export type GoalView = "active" | "inactive" | "done";

export type GoalCounts = Record<GoalView, number>;

export type GoalCursor = { order: number; id: string };

/** One `{"goals":1}` read: selected rows and the goals above them. */
export type GoalRead = {
  counts: GoalCounts;
  rows: GoalNode[];
  threads: { thread: string; goal: string | null }[];
  next: GoalCursor | null;
};

/** One edit; mirrors `kuzz_core::goal_tree::GoalOp`. */
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
  | { op: "unlink"; thread: string }
  | { op: "drop_inactive" };

export type GoalEdit = {
  eventId: string;
  createdAt: number;
  author: string;
  ops: GoalOp[];
};

export function getGoalPage(
  channelId: string,
  view: GoalView,
  cursor: GoalCursor | null = null,
): Promise<GoalRead> {
  return invokeTauri<GoalRead>("get_goals", { channelId, view, cursor });
}

/** The whole active list: every screen of the room reads it. */
export async function getActiveGoals(channelId: string): Promise<GoalRead> {
  const read = await getGoalPage(channelId, "active");
  // Every page repeats the goals above its own.
  const rows = new Map(read.rows.map((row) => [row.id, row]));
  let next = read.next;
  while (next) {
    const page = await getGoalPage(channelId, "active", next);
    for (const row of page.rows) {
      const matched = row.matched || rows.get(row.id)?.matched === true;
      rows.set(row.id, { ...row, matched });
    }
    next = page.next;
  }
  return { ...read, rows: [...rows.values()], next: null };
}

export function getThreadGoals(
  channelId: string,
  threads: string[],
): Promise<GoalRead> {
  return invokeTauri<GoalRead>("get_thread_goals", { channelId, threads });
}

/** Apply edits as one event: all of them or none. */
export async function applyGoalOps(
  channelId: string,
  ops: GoalOp[],
): Promise<{ eventId: string }> {
  const raw = await invokeTauri<{ event_id: string }>("apply_goal_ops", {
    channelId,
    ops,
  });
  return { eventId: raw.event_id };
}

export async function getGoalHistory(channelId: string): Promise<GoalEdit[]> {
  const raw = await invokeTauri<{
    edits: {
      event_id: string;
      created_at: number;
      author: string;
      ops: GoalOp[] | null;
    }[];
  }>("get_goal_history", { channelId, limit: 30 });
  return raw.edits.map((edit) => ({
    eventId: edit.event_id,
    createdAt: edit.created_at,
    author: edit.author,
    ops: edit.ops ?? [],
  }));
}

/** A fresh node id in the same shape as `kuzz_core::goal_tree::new_node_id`. */
export function newGoalId(): string {
  const bytes = new Uint8Array(8);
  crypto.getRandomValues(bytes);
  return `g_${Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("")}`;
}
