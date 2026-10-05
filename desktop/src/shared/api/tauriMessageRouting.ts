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

/** One routing call: a batch of the owner's sends in one channel. */
export type RouteMessageInput = {
  /** Oldest first; Rust routes at most 12. */
  messages: RouterNewMessage[];
  roster: RouterRosterEntry[];
  humans: string[];
  phase: RoutePhase;
  channelId: string | null;
  /** Earlier messages in the same conversation, oldest first. */
  recent?: RouterRecentMessage[];
  /** This desktop's deliveries in the channel, last 30 minutes. */
  prior?: RouterPriorDelivery[];
  /** Agents mid-turn right now. */
  working?: string[];
};

export type RouterNewMessage = {
  /** Event id; comes back in `RouteGroup.messageIds`, never prompted. */
  id: string;
  text: string;
  /** The thread root's text when the send is a reply. */
  threadRoot: string | null;
  /** Agents `@mentioned` in the message (soft: the router judges them). */
  mentioned: string[];
};

/** An earlier delivery a new message may continue, amend, or cancel. */
export type RouterPriorDelivery = {
  id: string;
  agents: string[];
  text: string;
};

/** One cached earlier message given to the router as context. */
export type RouterRecentMessage = {
  pubkey: string;
  /** Written by this desktop's owner. */
  isOwner: boolean;
  content: string;
  createdAt: number;
};

export type RouterSkipReason =
  | "not-configured"
  | "timeout"
  | "provider-error"
  | "bad-output";

export type RouteRelation = "new" | "continue" | "amend" | "cancel";

/** Messages of the batch that go to the same agents together. */
export type RouteGroup = {
  messageIds: string[];
  /** Empty: nobody acts (small talk, a human, no fitting agent). */
  pubkeys: string[];
  relation: RouteRelation;
  /** The earlier delivery's event id for any relation but `new`. */
  of: string | null;
};

export type RouteMessageResult =
  | { decision: "routed"; groups: RouteGroup[] }
  | { decision: "skipped"; reason: RouterSkipReason };

/**
 * A follow-up delivery's `buzz:route` note: the agent sees which earlier
 * message it relates to and the thread to answer in.
 */
export type RouteNote = {
  relation: Exclude<RouteRelation, "new">;
  of: string;
  threadRoot: string;
};

export async function routeMessage(
  input: RouteMessageInput,
): Promise<RouteMessageResult> {
  return invokeTauri<RouteMessageResult>("route_message", { input });
}

/**
 * `api-key` routes call the provider over HTTP with a Providers-tab key;
 * `subscription` routes run the official CLI (`codex exec`, `claude -p`) on
 * its own sign-in, which is slower (~5 s a call).
 */
export type TaskModelRouteKind = "api-key" | "subscription";

/** One route a task can use, e.g. "Codex (ChatGPT subscription)". */
export type TaskModelProviderOption = {
  id: string;
  label: string;
  kind: TaskModelRouteKind;
  /** Callable now: key saved, or CLI installed and signed in. */
  ready: boolean;
  /** Why not, e.g. "Sign in to Codex"; null when ready. */
  unavailableReason: string | null;
  defaultModel: string;
  /** Suggested models for a subscription route; empty for API keys. */
  models: string[];
  /** How long Enter waits for a routing answer on this route. */
  sendWaitMs: number;
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
  /** Enter's wait budget on the effective route. */
  sendWaitMs: number;
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
