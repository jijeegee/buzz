import { stringify as yamlStringify, parse as yamlParse } from "yaml";

import { type MessageKey, t } from "@/shared/i18n";
import {
  parsePubkeyInput as parseCanonicalPubkey,
  safeNpub,
} from "@/shared/lib/nostrUtils";

import { cronExpressionError } from "./cronExpression";
import {
  formatDurationSeconds,
  parseDurationSeconds,
} from "./workflowDuration";

export const TRIGGER_TYPES = [
  "message_posted",
  "reaction_added",
  "diff_posted",
  "webhook",
  "schedule",
] as const;
export type TriggerType = (typeof TRIGGER_TYPES)[number];

export function supportsMessageTextCondition(
  triggerType: TriggerType,
): boolean {
  return triggerType === "message_posted" || triggerType === "diff_posted";
}

export const SELECTABLE_TRIGGER_TYPES = [
  "message_posted",
  "reaction_added",
  "diff_posted",
  "webhook",
  "schedule",
] as const satisfies readonly TriggerType[];

export const ACTION_TYPES = [
  "delay",
  "send_message",
  "send_dm",
  "call_webhook",
  "request_approval",
  "add_reaction",
  "set_channel_topic",
] as const;
export type ActionType = (typeof ACTION_TYPES)[number];

export const SELECTABLE_ACTION_TYPES = [
  "send_message",
  "delay",
  "call_webhook",
] as const satisfies readonly ActionType[];

export type TriggerConfig = {
  on: TriggerType;
  filter?: string;
  emoji?: string;
  cron?: string;
  interval?: string;
};

export type HeaderFormState = {
  id: string;
  key: string;
  value: string;
};

export type StepFormState = {
  id: string;
  name?: string;
  action: ActionType;
  condition?: string;
  timeoutSecs?: string;
  duration?: string;
  text?: string;
  channel?: string;
  replyInThread?: boolean;
  to?: string;
  url?: string;
  method?: string;
  headers?: HeaderFormState[];
  body?: string;
  emoji?: string;
  topic?: string;
  from?: string;
  message?: string;
  timeout?: string;
};

export type WorkflowFormState = {
  name: string;
  description: string;
  enabled: boolean;
  trigger: TriggerConfig;
  steps: StepFormState[];
};

export const DEFAULT_FORM_STATE: WorkflowFormState = {
  name: "",
  description: "",
  enabled: true,
  trigger: { on: "message_posted" },
  steps: [],
};

/** Language-file keys of the trigger type names; translate where shown. */
export const TRIGGER_LABEL_KEYS: Record<TriggerType, MessageKey> = {
  message_posted: "workflows.triggerType.messagePosted",
  reaction_added: "workflows.triggerType.reactionAdded",
  diff_posted: "workflows.triggerType.diffPosted",
  webhook: "workflows.triggerType.webhook",
  schedule: "workflows.triggerType.schedule",
};

/** Language-file keys of the action type names; translate where shown. */
export const ACTION_LABEL_KEYS: Record<ActionType, MessageKey> = {
  delay: "workflows.actionType.delay",
  send_message: "workflows.actionType.sendMessage",
  send_dm: "workflows.actionType.sendDm",
  call_webhook: "workflows.actionType.callWebhook",
  request_approval: "workflows.actionType.requestApproval",
  add_reaction: "workflows.actionType.addReaction",
  set_channel_topic: "workflows.actionType.setChannelTopic",
};

export function triggerTypeLabel(type: TriggerType): string {
  return t(TRIGGER_LABEL_KEYS[type]);
}

export function actionTypeLabel(type: ActionType): string {
  return t(ACTION_LABEL_KEYS[type]);
}

const EXACT_HEX_64 = /^[0-9a-fA-F]{64}$/;

/**
 * Display form of a DM recipient (`to`) or approver (`from`): an exact 64-char
 * hex pubkey from stored YAML is shown as its npub; npubs, templates
 * (`{{trigger.author}}`), roles, and any other free text pass through
 * unchanged. Encoding failures (non-canonical hex) keep the stored value.
 */
function keyDisplayValue(value: string | undefined): string | undefined {
  if (value === undefined || !EXACT_HEX_64.test(value)) return value;
  return safeNpub(value.toLowerCase()) ?? value;
}

/**
 * Storage form: an exact npub decodes back to the canonical hex pubkey so the
 * wire YAML keeps its machine-readable hex contract. Hex, templates, roles,
 * and any other free text pass through unchanged.
 */
function keyStorageValue(value: string): string {
  if (!value.trim().toLowerCase().startsWith("npub1")) return value;
  return parseCanonicalPubkey(value) ?? value;
}

function toHeaderRows(
  headers: unknown,
  stepId: string,
): HeaderFormState[] | undefined {
  if (!headers || typeof headers !== "object" || Array.isArray(headers)) {
    return undefined;
  }

  const rows = Object.entries(headers).map(([key, value], index) => ({
    id: `${stepId}_header_${index + 1}`,
    key,
    value: typeof value === "string" ? value : String(value),
  }));

  return rows.length > 0 ? rows : undefined;
}

function headersToRecord(
  headers: HeaderFormState[] | undefined,
): Record<string, string> | undefined {
  if (!headers) return undefined;

  const entries = headers
    .map(({ key, value }) => [key.trim(), value] as const)
    .filter(([key]) => key.length > 0);

  if (entries.length === 0) return undefined;
  return Object.fromEntries(entries);
}

function parseTimeoutSecs(timeoutSecs: string | undefined): number | undefined {
  if (!timeoutSecs) return undefined;
  const parsed = parseDurationSeconds(timeoutSecs);
  return parsed !== null && parsed > 0 ? parsed : undefined;
}

function actionFieldsForStep(step: StepFormState): Record<string, unknown> {
  const fields: Record<string, unknown> = {};
  if (step.name?.trim()) fields.name = step.name.trim();
  if (step.condition?.trim()) fields.if = step.condition.trim();
  const timeoutSecs = parseTimeoutSecs(step.timeoutSecs);
  if (timeoutSecs !== undefined) fields.timeout_secs = timeoutSecs;

  switch (step.action) {
    case "delay":
      if (step.duration) fields.duration = step.duration;
      break;
    case "send_message":
      if (step.text) fields.text = step.text;
      if (step.channel) fields.channel = step.channel;
      if (step.replyInThread) fields.reply_in_thread = true;
      break;
    case "send_dm":
      if (step.to) fields.to = keyStorageValue(step.to);
      if (step.text) fields.text = step.text;
      break;
    case "call_webhook":
      if (step.url) fields.url = step.url;
      fields.method = step.method || "POST";
      {
        const headers = headersToRecord(step.headers);
        if (headers) fields.headers = headers;
      }
      if (step.body) fields.body = step.body;
      break;
    case "request_approval":
      if (step.from) fields.from = keyStorageValue(step.from);
      if (step.message) fields.message = step.message;
      if (step.timeout) fields.timeout = step.timeout;
      break;
    case "add_reaction":
      if (step.emoji) fields.emoji = step.emoji;
      break;
    case "set_channel_topic":
      if (step.topic) fields.topic = step.topic;
      break;
  }
  return fields;
}

export function isThreadReplyEligibleTrigger(trigger: TriggerType): boolean {
  return trigger !== "webhook" && trigger !== "schedule";
}

export function withTriggerType(
  state: WorkflowFormState,
  triggerType: TriggerType,
): WorkflowFormState {
  return {
    ...state,
    trigger: { on: triggerType },
    // Clear threaded-reply state on every step, not just send_message ones:
    // a hidden `replyInThread` on a step whose action was changed away from
    // send_message would otherwise resurrect when the action is switched back.
    steps: isThreadReplyEligibleTrigger(triggerType)
      ? state.steps
      : state.steps.map((step) =>
          step.replyInThread ? { ...step, replyInThread: false } : step,
        ),
  };
}

export function formStateToYaml(state: WorkflowFormState): string {
  const trigger: Record<string, unknown> = { on: state.trigger.on };
  if (
    (state.trigger.on === "message_posted" ||
      state.trigger.on === "diff_posted" ||
      state.trigger.on === "reaction_added") &&
    state.trigger.filter
  ) {
    trigger.filter = state.trigger.filter;
  }
  if (state.trigger.on === "reaction_added" && state.trigger.emoji) {
    trigger.emoji = state.trigger.emoji;
  }
  if (state.trigger.on === "schedule") {
    if (state.trigger.cron) {
      trigger.cron = state.trigger.cron;
    } else if (state.trigger.interval) {
      trigger.interval = state.trigger.interval;
    }
  }

  const steps = state.steps.map((step) => ({
    id: step.id,
    action: step.action,
    ...actionFieldsForStep(step),
  }));

  const workflow: Record<string, unknown> = {
    name: state.name,
    trigger,
    steps,
  };

  if (state.description.trim()) {
    workflow.description = state.description.trim();
  }
  if (!state.enabled) {
    workflow.enabled = false;
  }

  return yamlStringify(workflow);
}

const STEP_ID_PATTERN = /^step_(\d+)$/;

export function nextStepId(existingSteps: StepFormState[]): string {
  const existingIds = new Set(existingSteps.map((s) => s.id));
  let maxN = 0;
  for (const id of existingIds) {
    const match = STEP_ID_PATTERN.exec(id);
    if (match) maxN = Math.max(maxN, Number(match[1]));
  }
  let n = maxN + 1;
  while (existingIds.has(`step_${n}`)) n++;
  return `step_${n}`;
}

const TOP_LEVEL_KEYS = new Set([
  "name",
  "description",
  "enabled",
  "trigger",
  "steps",
]);
const TRIGGER_KEYS: Record<TriggerType, ReadonlySet<string>> = {
  message_posted: new Set(["on", "filter"]),
  reaction_added: new Set(["on", "emoji", "filter"]),
  diff_posted: new Set(["on", "filter"]),
  webhook: new Set(["on"]),
  schedule: new Set(["on", "cron", "interval"]),
};
const COMMON_STEP_KEYS = ["id", "name", "action", "if", "timeout_secs"];
const ACTION_STEP_KEYS: Record<ActionType, ReadonlySet<string>> = {
  delay: new Set([...COMMON_STEP_KEYS, "duration"]),
  send_message: new Set([
    ...COMMON_STEP_KEYS,
    "text",
    "channel",
    "reply_in_thread",
  ]),
  send_dm: new Set([...COMMON_STEP_KEYS, "to", "text"]),
  call_webhook: new Set([
    ...COMMON_STEP_KEYS,
    "url",
    "method",
    "headers",
    "body",
  ]),
  request_approval: new Set([
    ...COMMON_STEP_KEYS,
    "from",
    "message",
    "timeout",
  ]),
  add_reaction: new Set([...COMMON_STEP_KEYS, "emoji"]),
  set_channel_topic: new Set([...COMMON_STEP_KEYS, "topic"]),
};
const REQUIRED_ACTION_STRING_KEYS: Record<ActionType, readonly string[]> = {
  delay: ["duration"],
  send_message: ["text"],
  send_dm: ["to", "text"],
  call_webhook: ["url"],
  request_approval: ["from", "message"],
  add_reaction: ["emoji"],
  set_channel_topic: ["topic"],
};
const OPTIONAL_ACTION_STRING_KEYS: Record<ActionType, readonly string[]> = {
  delay: [],
  send_message: ["channel"],
  send_dm: [],
  call_webhook: ["method", "body"],
  request_approval: ["timeout"],
  add_reaction: [],
  set_channel_topic: [],
};
const WEBHOOK_METHODS = new Set(["POST", "GET", "PUT", "PATCH", "DELETE"]);
const STEP_ID_PATTERN_STRICT = /^[A-Za-z0-9_]{1,64}$/;

type UnknownRecord = Record<string, unknown>;

function objectRecord(value: unknown): UnknownRecord | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? (value as UnknownRecord)
    : null;
}

function unknownKey(
  record: UnknownRecord,
  allowed: ReadonlySet<string>,
): string | null {
  return Object.keys(record).find((key) => !allowed.has(key)) ?? null;
}

function requireNonEmptyString(
  record: UnknownRecord,
  key: string,
  label: string,
): string | { error: string } {
  const value = record[key];
  if (typeof value !== "string" || value.length === 0) {
    return { error: t("workflows.yamlError.nonEmptyString", { field: label }) };
  }
  return value;
}

function optionalOwnedStringError(
  record: UnknownRecord,
  key: string,
  label: string,
  // Schedule triggers point every refusal at the YAML editor.
  pointToYamlEditor = false,
): string | null {
  const value = record[key];
  if (value === undefined) return null;
  if (typeof value !== "string") {
    return pointToYamlEditor
      ? t("workflows.yamlError.stringUseYaml", { field: label })
      : t("workflows.yamlError.string", { field: label });
  }
  if (value.length === 0) {
    return t("workflows.yamlError.emptyInForm", { field: label });
  }
  return null;
}

export function yamlToFormState(
  yaml: string,
): { ok: true; state: WorkflowFormState } | { ok: false; error: string } {
  try {
    const parsed = objectRecord(yamlParse(yaml));
    if (!parsed) {
      return { ok: false, error: t("workflows.yamlError.notObject") };
    }

    const topUnknown = unknownKey(parsed, TOP_LEVEL_KEYS);
    if (topUnknown) {
      return {
        ok: false,
        error: t("workflows.yamlError.unsupportedField", { field: topUnknown }),
      };
    }
    if (typeof parsed.name !== "string") {
      return {
        ok: false,
        error: t("workflows.yamlError.string", { field: "name" }),
      };
    }
    if (parsed.description !== undefined) {
      if (typeof parsed.description !== "string") {
        return {
          ok: false,
          error: t("workflows.yamlError.string", { field: "description" }),
        };
      }
      if (
        parsed.description.length === 0 ||
        parsed.description.trim() !== parsed.description
      ) {
        return {
          ok: false,
          error: t("workflows.yamlError.descriptionWhitespace"),
        };
      }
    }
    if (parsed.enabled !== undefined && typeof parsed.enabled !== "boolean") {
      return {
        ok: false,
        error: t("workflows.yamlError.boolean", { field: "enabled" }),
      };
    }

    const rawTrigger = objectRecord(parsed.trigger);
    if (!rawTrigger || typeof rawTrigger.on !== "string") {
      return {
        ok: false,
        error: t("workflows.yamlError.required", { field: "trigger.on" }),
      };
    }
    if (!TRIGGER_TYPES.includes(rawTrigger.on as TriggerType)) {
      return {
        ok: false,
        error: t("workflows.yamlError.unsupportedTriggerType", {
          type: rawTrigger.on,
        }),
      };
    }
    const triggerOn = rawTrigger.on as TriggerType;
    const triggerUnknown = unknownKey(rawTrigger, TRIGGER_KEYS[triggerOn]);
    if (triggerUnknown) {
      return {
        ok: false,
        error: t("workflows.yamlError.unsupportedTriggerField", {
          trigger: triggerOn,
          field: triggerUnknown,
        }),
      };
    }
    for (const key of ["filter", "emoji", "cron", "interval"] as const) {
      const error = optionalOwnedStringError(
        rawTrigger,
        key,
        `trigger.${key}`,
        triggerOn === "schedule",
      );
      if (error) return { ok: false, error };
    }
    if (triggerOn === "schedule") {
      const hasCron = rawTrigger.cron !== undefined;
      const hasInterval = rawTrigger.interval !== undefined;
      if (hasCron === hasInterval) {
        return {
          ok: false,
          error: hasCron
            ? t("workflows.yamlError.scheduleBoth")
            : t("workflows.yamlError.scheduleNeither"),
        };
      }
      if (typeof rawTrigger.cron === "string") {
        const error = cronExpressionError(rawTrigger.cron);
        if (error) {
          return {
            ok: false,
            error: t("workflows.yamlError.unsupportedCron", { error }),
          };
        }
      }
    }
    const trigger: TriggerConfig = {
      on: triggerOn,
      filter: rawTrigger.filter as string | undefined,
      emoji: rawTrigger.emoji as string | undefined,
      cron: rawTrigger.cron as string | undefined,
      interval: rawTrigger.interval as string | undefined,
    };

    if (!Array.isArray(parsed.steps)) {
      return { ok: false, error: t("workflows.yamlError.stepsNotList") };
    }
    const ids = new Set<string>();
    const steps: StepFormState[] = [];
    for (const [index, value] of parsed.steps.entries()) {
      const number = index + 1;
      const step = objectRecord(value);
      if (!step) {
        return {
          ok: false,
          error: t("workflows.yamlError.stepNotObject", { number }),
        };
      }
      if (
        typeof step.id !== "string" ||
        !STEP_ID_PATTERN_STRICT.test(step.id)
      ) {
        return {
          ok: false,
          error: t("workflows.yamlError.stepIdInvalid", { number }),
        };
      }
      if (ids.has(step.id)) {
        return {
          ok: false,
          error: t("workflows.yamlError.duplicateStepId", { id: step.id }),
        };
      }
      ids.add(step.id);

      if (
        typeof step.action !== "string" ||
        !ACTION_TYPES.includes(step.action as ActionType)
      ) {
        return {
          ok: false,
          error: t("workflows.yamlError.unsupportedActionType", {
            type: String(step.action),
          }),
        };
      }
      const action = step.action as ActionType;
      const stepUnknown = unknownKey(step, ACTION_STEP_KEYS[action]);
      if (stepUnknown) {
        return {
          ok: false,
          error: t("workflows.yamlError.unsupportedStepField", {
            action,
            field: stepUnknown,
          }),
        };
      }
      if (step.if !== undefined) {
        return {
          ok: false,
          error: t("workflows.yamlError.stepConditions"),
        };
      }
      const nameError = optionalOwnedStringError(
        step,
        "name",
        t("workflows.yamlError.stepField", { number, field: "name" }),
      );
      if (nameError) return { ok: false, error: nameError };
      if (typeof step.name === "string" && step.name.trim() !== step.name) {
        return {
          ok: false,
          error: t("workflows.yamlError.stepNameWhitespace", { number }),
        };
      }
      if (
        step.timeout_secs !== undefined &&
        (!Number.isSafeInteger(step.timeout_secs) ||
          (step.timeout_secs as number) <= 0)
      ) {
        return {
          ok: false,
          error: t("workflows.yamlError.stepTimeoutSecs", { number }),
        };
      }

      for (const key of REQUIRED_ACTION_STRING_KEYS[action]) {
        const required = requireNonEmptyString(
          step,
          key,
          t("workflows.yamlError.stepField", { number, field: key }),
        );
        if (typeof required !== "string")
          return { ok: false, error: required.error };
      }
      for (const key of OPTIONAL_ACTION_STRING_KEYS[action]) {
        const error = optionalOwnedStringError(
          step,
          key,
          t("workflows.yamlError.stepField", { number, field: key }),
        );
        if (error) return { ok: false, error };
      }
      if (
        action === "call_webhook" &&
        step.method !== undefined &&
        !WEBHOOK_METHODS.has(step.method as string)
      ) {
        return {
          ok: false,
          error: t("workflows.yamlError.unsupportedWebhookMethod", {
            method: String(step.method),
          }),
        };
      }
      if (step.headers !== undefined) {
        const headers = objectRecord(step.headers);
        if (
          !headers ||
          Object.keys(headers).length === 0 ||
          Object.values(headers).some((header) => typeof header !== "string")
        ) {
          return {
            ok: false,
            error: t("workflows.yamlError.webhookHeadersShape"),
          };
        }
        const unsafeHeader = Object.keys(headers).find(
          (key) => key.length === 0 || key.trim() !== key,
        );
        if (unsafeHeader !== undefined) {
          return {
            ok: false,
            error: t("workflows.yamlError.webhookHeaderNames"),
          };
        }
      }

      if (step.reply_in_thread !== undefined) {
        if (typeof step.reply_in_thread !== "boolean") {
          return {
            ok: false,
            error: t("workflows.yamlError.stepReplyInThreadBoolean", {
              number,
            }),
          };
        }
        if (step.reply_in_thread && !isThreadReplyEligibleTrigger(triggerOn)) {
          return {
            ok: false,
            error: t("workflows.yamlError.replyInThreadUnsupported", {
              trigger: triggerOn,
            }),
          };
        }
      }

      steps.push({
        id: step.id,
        name: step.name as string | undefined,
        action,
        timeoutSecs:
          step.timeout_secs === undefined
            ? undefined
            : formatDurationSeconds(step.timeout_secs as number),
        duration: step.duration as string | undefined,
        text: step.text as string | undefined,
        channel: step.channel as string | undefined,
        replyInThread: step.reply_in_thread === true,
        to: keyDisplayValue(step.to as string | undefined),
        url: step.url as string | undefined,
        method: step.method as string | undefined,
        headers: toHeaderRows(step.headers, step.id),
        body: step.body as string | undefined,
        emoji: step.emoji as string | undefined,
        topic: step.topic as string | undefined,
        from: keyDisplayValue(step.from as string | undefined),
        message: step.message as string | undefined,
        timeout: step.timeout as string | undefined,
      });
    }

    return {
      ok: true,
      state: {
        name: parsed.name,
        description: (parsed.description as string | undefined) ?? "",
        enabled: parsed.enabled !== false,
        trigger,
        steps,
      },
    };
  } catch (error) {
    return {
      ok: false,
      error:
        error instanceof Error
          ? error.message
          : t("workflows.yamlError.invalid"),
    };
  }
}
