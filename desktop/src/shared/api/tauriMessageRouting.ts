import { invokeTauri } from "@/shared/api/tauri";

/**
 * Smart routing (channel routing mode `desktop-router`) and the Settings ›
 * Models › Task models entry that picks its model. Rust owns the prompt,
 * caps, model call, and comparison log; this module only carries data.
 */

export type RouterRosterEntry = {
  pubkey: string;
  name: string;
  description: string | null;
};

/** `preview` = debounced while typing; `send` = resolved on Enter. */
export type RoutePhase = "preview" | "send";

export type RouteMessageInput = {
  message: string;
  /** The thread root's text when the send is a reply. */
  threadRoot: string | null;
  roster: RouterRosterEntry[];
  humans: string[];
  phase: RoutePhase;
  channelId: string | null;
};

export type RouterSkipReason =
  | "not-configured"
  | "timeout"
  | "provider-error"
  | "bad-output";

export type RouteMessageResult =
  | { decision: "assigned"; pubkeys: string[] }
  /** The model chose nobody (small talk, a human, no fitting agent). */
  | { decision: "none" }
  | { decision: "skipped"; reason: RouterSkipReason };

export async function routeMessage(
  input: RouteMessageInput,
): Promise<RouteMessageResult> {
  return invokeTauri<RouteMessageResult>("route_message", { input });
}

export type TaskModelProviderOption = {
  id: string;
  label: string;
  hasKey: boolean;
  defaultModel: string;
};

/** One app task's model state for Settings › Models › Task models. */
export type TaskModelStatus = {
  taskId: string;
  /** Saved choices; null = automatic. */
  provider: string | null;
  model: string | null;
  /** What a call would use right now (null while not ready). */
  effectiveProvider: string | null;
  effectiveModel: string | null;
  /** e.g. "Claude Haiku 4.5". */
  modelLabel: string | null;
  ready: boolean;
  notReadyReason: string | null;
  providers: TaskModelProviderOption[];
};

export const MESSAGE_ROUTING_TASK_ID = "message-routing";

export async function getTaskModels(): Promise<TaskModelStatus[]> {
  return invokeTauri<TaskModelStatus[]>("get_task_models");
}

/** One save per user action; null provider/model = automatic. */
export async function setTaskModel(
  taskId: string,
  provider: string | null,
  model: string | null,
): Promise<TaskModelStatus[]> {
  return invokeTauri<TaskModelStatus[]>("set_task_model", {
    taskId,
    provider,
    model,
  });
}
