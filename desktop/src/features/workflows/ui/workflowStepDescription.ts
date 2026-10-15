import {
  formatDurationSecondsVerbose,
  parseDurationSeconds,
} from "./workflowDuration";
import { ACTION_LABELS } from "./workflowFormTypes";
import type { StepFormState } from "./workflowFormTypes";

const MAX_DETAIL_LENGTH = 42;

function compact(value: string): string {
  const normalized = value.trim().replaceAll(/\s+/g, " ");
  return normalized.length > MAX_DETAIL_LENGTH
    ? `${normalized.slice(0, MAX_DETAIL_LENGTH - 3)}...`
    : normalized;
}

function quoted(value: string | undefined): string | null {
  const normalized = value ? compact(value) : "";
  return normalized ? `“${normalized}”` : null;
}

// DM/approver references are pubkeys — 64-char hex in any case, or a
// canonical lowercase npub — or freeform role/template text. A key identity
// has no resolvable name here, so it renders as "someone", never key text.
const KEY_REFERENCE = /^(?:[0-9a-f]{64}|npub1[0-9a-z]+)$/i;

function destination(value: string | undefined): string | null {
  const normalized = value?.trim();
  if (!normalized) return null;
  return KEY_REFERENCE.test(normalized) ? "someone" : compact(normalized);
}

function configuredStepDetail(
  step: StepFormState,
  channelLabel?: string,
): string | null {
  switch (step.action) {
    case "delay": {
      const duration = step.duration?.trim();
      if (!duration) return null;
      const seconds = parseDurationSeconds(duration);
      return compact(
        seconds === null ? duration : formatDurationSecondsVerbose(seconds),
      );
    }
    case "send_message": {
      const text = quoted(step.text);
      const channel = channelLabel ? `#${channelLabel}` : null;
      if (text && channel) return `${text} in ${channel}`;
      return text ?? channel;
    }
    case "send_dm": {
      const text = quoted(step.text);
      const recipient = destination(step.to);
      if (text && recipient) return `${text} to ${recipient}`;
      return text ?? recipient;
    }
    case "call_webhook": {
      const url = step.url?.trim();
      if (!url) return null;
      return `${step.method?.trim() || "POST"} ${compact(url)}`;
    }
    case "request_approval": {
      const message = quoted(step.message);
      const approver = destination(step.from);
      if (message && approver) return `${message} from ${approver}`;
      return message ?? approver;
    }
    case "add_reaction":
      return step.emoji?.trim() || null;
    case "set_channel_topic":
      return quoted(step.topic);
  }
}

/** Build the concise action summary rendered on a workflow step node. */
export function workflowStepDescription(
  step: StepFormState,
  options: { channelLabel?: string; includeName?: boolean } = {},
): string {
  const name = options.includeName === false ? undefined : step.name?.trim();
  const detail = configuredStepDetail(step, options.channelLabel);
  if (name && detail) return `${name} · ${detail}`;
  return name || detail || ACTION_LABELS[step.action];
}
