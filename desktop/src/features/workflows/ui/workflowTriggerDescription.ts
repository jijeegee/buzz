import { type MessageKey, t } from "@/shared/i18n";
import { truncatePubkey, unknownUserLabel } from "@/shared/lib/pubkey";
import { parseConditionExpressions } from "./workflowConditionExpression";
import { triggerTypeLabel } from "./workflowFormTypes";
import type { ParsedConditionExpression } from "./workflowConditionExpression";
import type { TriggerConfig } from "./workflowFormTypes";

const EVENT_PHRASE_KEYS = {
  diff_posted: "workflows.triggerDescription.diffPosted",
  message_posted: "workflows.triggerDescription.messagePosted",
  reaction_added: "workflows.triggerDescription.reactionAdded",
} as const satisfies Record<string, MessageKey>;

type DescribedEvent = keyof typeof EVENT_PHRASE_KEYS;

/** The event name without "posted", used before text conditions. */
function eventSubject(event: DescribedEvent): string {
  switch (event) {
    case "diff_posted":
      return t("workflows.triggerDescription.diffSubject");
    case "message_posted":
      return t("workflows.triggerDescription.messageSubject");
    case "reaction_added":
      return t(EVENT_PHRASE_KEYS.reaction_added);
  }
}

function byAuthor(description: string, author: string, except: boolean) {
  return except
    ? t("workflows.triggerDescription.byAnyoneExcept", { description, author })
    : t("workflows.triggerDescription.byAuthor", { description, author });
}

function emojiReaction(emoji: string, except: boolean): string {
  return except
    ? t("workflows.triggerDescription.anyReactionExcept", { emoji })
    : t("workflows.triggerDescription.emojiReactionAdded", { emoji });
}

function toMessage(description: string, message: string, except: boolean) {
  return except
    ? t("workflows.triggerDescription.anywhereExcept", { description, message })
    : t("workflows.triggerDescription.toMessage", { description, message });
}

function authorReference(
  authorLabel?: string,
  authorLoading?: boolean,
): string {
  if (authorLoading) return t("workflows.triggerDescription.loadingAuthor");
  // The trigger author is a pubkey identity; an unresolved author renders the
  // generic unknown-user label, never key text.
  return authorLabel ?? unknownUserLabel();
}

function quotedValue(value: string): string {
  const normalized = value.trim().replaceAll(/\s+/g, " ");
  const abbreviated =
    normalized.length > 36 ? `${normalized.slice(0, 33)}...` : normalized;
  return `“${abbreviated}”`;
}

function messageReference(
  condition: ParsedConditionExpression,
  messageLabel?: string,
  messageLoading?: boolean,
): string {
  if (messageLoading) return t("workflows.triggerDescription.loadingMessage");
  // The referenced message is an event id, not a pubkey identity: it keeps
  // the generic hex truncation.
  return messageLabel
    ? quotedValue(messageLabel)
    : truncatePubkey(condition.value);
}

function textConditionDescription(
  subject: string,
  condition: ParsedConditionExpression,
): string {
  const value = quotedValue(condition.value);
  switch (condition.operator) {
    case "contains":
      return t("workflows.triggerDescription.textContains", { subject, value });
    case "not_contains":
      return t("workflows.triggerDescription.textNotContains", {
        subject,
        value,
      });
    case "starts_with":
      return t("workflows.triggerDescription.textStartsWith", {
        subject,
        value,
      });
    case "ends_with":
      return t("workflows.triggerDescription.textEndsWith", { subject, value });
    case "equals":
      return t("workflows.triggerDescription.textEquals", { subject, value });
    case "not_equals":
      return t("workflows.triggerDescription.textNotEquals", {
        subject,
        value,
      });
    case "is_not_empty":
      return t("workflows.triggerDescription.textNotEmpty", { subject });
    case "is_empty":
      return t("workflows.triggerDescription.textEmpty", { subject });
  }
}

/** Text conditions that end with the event's "posted" wording. */
function endsWithPosted(condition: ParsedConditionExpression): boolean {
  return (
    condition.operator === "equals" ||
    condition.operator === "not_equals" ||
    condition.operator === "is_not_empty" ||
    condition.operator === "is_empty"
  );
}

export type WorkflowAuthorDescriptionSegments = {
  prefix: string;
  suffix: string;
};

/** Locate the generated author attribution without matching quoted user copy. */
export function splitWorkflowAuthorDescription(
  description: string,
  label: string,
): WorkflowAuthorDescriptionSegments | null {
  const markers = [
    byAuthor("", label, true),
    byAuthor("", label, false),
  ].filter((marker) => marker.trim() !== label);

  for (const marker of markers) {
    let insideQuote = false;
    for (
      let index = 0;
      index <= description.length - marker.length;
      index += 1
    ) {
      const character = description[index];
      if (character === "“") insideQuote = true;
      if (character === "”") insideQuote = false;
      if (!insideQuote && description.startsWith(marker, index)) {
        const labelIndex = index + marker.length - label.length;
        return {
          prefix: description.slice(0, labelIndex).trimEnd(),
          suffix: description.slice(labelIndex + label.length).trimStart(),
        };
      }
    }
  }

  return null;
}

/** Remove a reaction value from compact copy when the node icon already shows it. */
export function compactWorkflowTriggerDescription(
  description: string,
  triggerEmoji?: string,
): string {
  if (!triggerEmoji) return description;
  const emojiPrefix = emojiReaction(triggerEmoji, false);
  return description.startsWith(emojiPrefix)
    ? `${t(EVENT_PHRASE_KEYS.reaction_added)}${description.slice(emojiPrefix.length)}`
    : description;
}

/** Build the concise trigger summary rendered on the workflow canvas. */
export function workflowTriggerDescription(
  trigger: TriggerConfig,
  options: {
    authorLoading?: boolean;
    authorLabel?: string;
    messageLabel?: string;
    messageLoading?: boolean;
    omitUnresolvedReferences?: boolean;
  } = {},
): string {
  if (!(trigger.on in EVENT_PHRASE_KEYS)) return triggerTypeLabel(trigger.on);
  const event = trigger.on as DescribedEvent;
  const eventPhrase = t(EVENT_PHRASE_KEYS[event]);
  const baseLabel = eventPhrase;
  const subject = eventSubject(event);

  const conditions = trigger.filter
    ? parseConditionExpressions(trigger.filter, trigger.on)
    : [];
  if (!conditions || conditions.length === 0) {
    return trigger.on === "reaction_added" && trigger.emoji
      ? eventPhrase
      : baseLabel;
  }

  const condition = conditions[0];

  if (conditions.length > 1) {
    const authorCondition = conditions.find(
      ({ field }) => field === "trigger_author",
    );
    const textCondition = conditions.find(
      ({ field }) => field === "trigger_text",
    );
    const emojiCondition = conditions.find(
      ({ field }) => field === "trigger_emoji",
    );
    const messageCondition = conditions.find(
      ({ field }) => field === "trigger_message_id",
    );
    let description = textCondition
      ? textConditionDescription(subject, textCondition)
      : eventPhrase;

    if (emojiCondition) {
      description = emojiReaction(
        emojiCondition.value,
        emojiCondition.operator === "not_equals",
      );
    }
    if (
      authorCondition &&
      (!options.omitUnresolvedReferences ||
        options.authorLabel ||
        options.authorLoading)
    ) {
      const author = authorReference(
        options.authorLabel,
        options.authorLoading,
      );
      const except = authorCondition.operator === "not_equals";
      // "Message by Carl contains …" reads better than "… contains … by Carl".
      description =
        textCondition && !emojiCondition && !endsWithPosted(textCondition)
          ? textConditionDescription(
              byAuthor(subject, author, except),
              textCondition,
            )
          : byAuthor(description, author, except);
    }
    if (
      messageCondition &&
      (!options.omitUnresolvedReferences ||
        options.messageLabel ||
        options.messageLoading)
    ) {
      const message = messageReference(
        messageCondition,
        options.messageLabel,
        options.messageLoading,
      );
      description = toMessage(
        description,
        message,
        messageCondition.operator === "not_equals",
      );
    }
    return description;
  }

  if (condition.field === "trigger_author") {
    if (
      options.omitUnresolvedReferences &&
      !options.authorLabel &&
      !options.authorLoading
    ) {
      return baseLabel;
    }
    const author = authorReference(options.authorLabel, options.authorLoading);
    return byAuthor(eventPhrase, author, condition.operator === "not_equals");
  }

  if (condition.field === "trigger_text") {
    return textConditionDescription(subject, condition);
  }

  if (condition.field === "trigger_emoji") {
    return emojiReaction(condition.value, condition.operator === "not_equals");
  }

  if (condition.field === "trigger_message_id") {
    if (
      options.omitUnresolvedReferences &&
      !options.messageLabel &&
      !options.messageLoading
    ) {
      return baseLabel;
    }
    const message = messageReference(
      condition,
      options.messageLabel,
      options.messageLoading,
    );
    return toMessage(
      t(EVENT_PHRASE_KEYS.reaction_added),
      message,
      condition.operator === "not_equals",
    );
  }

  return baseLabel;
}
