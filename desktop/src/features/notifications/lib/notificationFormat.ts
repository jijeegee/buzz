import { type MessageKey, t } from "@/shared/i18n";

const NOTIFICATION_BODY_MAX_LENGTH = 140;

/**
 * Resolve a channel's display label for use in notification titles.
 *
 * Returns `"#channelName"` when the channel is found and has a non-empty name,
 * or `null` when the channelId is absent or the channel is not yet in the list
 * (e.g. channels query hasn't resolved — the caller should fall back gracefully
 * rather than blocking the toast).
 */
export function resolveNotificationChannelLabel(
  channelId: string | null | undefined,
  channels: ReadonlyArray<{ id: string; name?: string | null }>,
): string | null {
  if (!channelId) return null;
  const channel = channels.find((c) => c.id === channelId);
  const name = channel?.name?.trim();
  return name ? `${name}` : null;
}

/**
 * Truncate notification body text to {@link NOTIFICATION_BODY_MAX_LENGTH}
 * characters, appending "..." when truncated.  Returns `fallback` when
 * `content` is blank after trimming.
 */
export function truncateNotificationBody(
  content: string,
  fallback: string,
): string {
  const trimmed = content.trim();
  if (trimmed.length === 0) return fallback;
  if (trimmed.length <= NOTIFICATION_BODY_MAX_LENGTH) return trimmed;
  return `${trimmed.slice(0, NOTIFICATION_BODY_MAX_LENGTH - 3).trimEnd()}...`;
}

/**
 * Format a notification title with optional channel context.
 *
 * - With a channel label: `"prefix in #channel"`
 * - Without: `"prefix"`
 */
export function formatNotificationTitle(opts: {
  prefix: string;
  channelLabel: string | null;
}): string {
  return opts.channelLabel
    ? t("notifications.titleInChannel", {
        prefix: opts.prefix,
        channel: opts.channelLabel,
      })
    : opts.prefix;
}

export type MessageNotificationSource =
  | "mention"
  | "approval"
  | "needs_action"
  | "dm"
  | "chat"
  | "thread_reply";

const MESSAGE_BODY_FALLBACKS: Record<MessageNotificationSource, MessageKey> = {
  mention: "notifications.body.needsAttention",
  approval: "notifications.body.approvalWaiting",
  needs_action: "notifications.body.needsAttention",
  dm: "notifications.body.newMessage",
  chat: "notifications.body.newMessage",
  thread_reply: "notifications.body.newReply",
};

/**
 * Canonical copy for every message-shaped desktop notification (home-feed
 * mentions and needs-action items, live DMs, live thread replies). All paths
 * format through here so sender attribution and fallbacks stay consistent:
 * the sender leads the title whenever their profile has resolved, and each
 * source degrades to neutral copy — never a raw pubkey — when it has not.
 *
 * `senderName` must already be a real human label (see
 * `senderNameFromSummary`); `channelName` is the raw channel name without
 * a `#` prefix.
 */
export function formatMessageNotification(opts: {
  source: MessageNotificationSource;
  senderName?: string | null;
  channelName?: string | null;
  content: string;
}): { title: string; body: string } {
  const { source, content } = opts;
  const senderName = opts.senderName?.trim() || null;
  const channelName = opts.channelName?.trim() || null;
  const body = truncateNotificationBody(
    content,
    t(MESSAGE_BODY_FALLBACKS[source]),
  );

  if (source === "dm") {
    return {
      title:
        senderName ?? channelName ?? t("notifications.title.directMessage"),
      body,
    };
  }
  // Ownerless chat: who said it, and in which chat.
  if (source === "chat") {
    const chat = channelName ?? t("notifications.title.chat");
    return {
      title: senderName
        ? t("notifications.title.senderInChat", { name: senderName, chat })
        : chat,
      body,
    };
  }

  return { title: messageTitle(source, senderName, channelName), body };
}

/** Whole-sentence titles: who did what, and where when the channel is known. */
function messageTitle(
  source: Exclude<MessageNotificationSource, "dm" | "chat">,
  name: string | null,
  channel: string | null,
): string {
  if (source === "mention") {
    if (name) {
      return channel
        ? t("notifications.title.mentionedYouIn", { name, channel })
        : t("notifications.title.mentionedYou", { name });
    }
    return channel
      ? t("notifications.title.mentionIn", { channel })
      : t("notifications.title.mention");
  }
  if (source === "approval") {
    if (name) {
      return channel
        ? t("notifications.title.requestedApprovalIn", { name, channel })
        : t("notifications.title.requestedApproval", { name });
    }
    return channel
      ? t("notifications.title.approvalRequestedIn", { channel })
      : t("notifications.title.approvalRequested");
  }
  if (source === "thread_reply") {
    if (name) {
      return channel
        ? t("notifications.title.repliedIn", { name, channel })
        : t("notifications.title.replied", { name });
    }
    return channel
      ? t("notifications.title.replyIn", { channel })
      : t("notifications.title.reply");
  }
  const prefix = name ?? t("notifications.title.needsAction");
  return formatNotificationTitle({ prefix, channelLabel: channel });
}
