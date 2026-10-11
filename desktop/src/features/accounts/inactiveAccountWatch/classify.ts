import { DM_NOTIFIABLE_EVENT_KINDS } from "@/features/channels/isDmNotifiableKind";
import { isThreadReply } from "@/features/messages/lib/threading";
import type { NotificationSettings } from "@/features/notifications/hooks";
import {
  hasMentionForEvent,
  shouldNotifyForEvent,
} from "@/features/notifications/lib/shouldNotify";
import type { ChannelType, RelayEvent } from "@/shared/api/types";

/** What a new event means for an account that is not the open one. */
export type WatchedEventEffect = {
  /** OS notification slot, or null for none. */
  notify: "dm" | "mention" | "thread_reply" | null;
  /** Counts toward the account's unread dot. */
  unread: boolean;
  /** Counts toward the account's mention number. */
  mention: boolean;
};

export type WatchedAccountContext = {
  /** The watched account, lowercase hex. */
  pubkey: string;
  settings: NotificationSettings;
  mutedChannelIds: ReadonlySet<string>;
  participatedRootIds: ReadonlySet<string>;
  followedRootIds: ReadonlySet<string>;
  authoredRootIds: ReadonlySet<string>;
  mutedRootIds: ReadonlySet<string>;
};

const NONE: WatchedEventEffect = {
  notify: null,
  unread: false,
  mention: false,
};

const DM_KINDS: ReadonlySet<number> = new Set(DM_NOTIFIABLE_EVENT_KINDS);

/**
 * The same rules the open account notifies by, applied to a watched one:
 * DMs (`useAppShellDesktopNotifications` dm path), mentions (the home-feed
 * mention path; `hasMentionForEvent`, so a reply's thread-participant p-tags
 * are not mentions), and replies in threads the account follows
 * (`useLiveChannelUpdates` thread-reply path). Each needs desktop
 * notifications and that slot on in the account's own settings. Other
 * channel messages only light the unread dot. `needs_action` comes from the
 * server feed and is not covered.
 */
export function classifyWatchedEvent(
  event: RelayEvent,
  channel: { id: string; channelType: ChannelType },
  context: WatchedAccountContext,
): WatchedEventEffect {
  const pubkey = context.pubkey.toLowerCase();
  if (event.pubkey.toLowerCase() === pubkey) return NONE;
  const isDm = channel.channelType === "dm";
  if (isDm && !DM_KINDS.has(event.kind)) return NONE;
  if (context.mutedChannelIds.has(channel.id) && !isDm) {
    // A muted channel still reports mentions, like the open account.
    if (!hasMentionForEvent(event, pubkey)) return NONE;
  }

  const unread = shouldNotifyForEvent(event, pubkey, {
    participatedRootIds: context.participatedRootIds,
    followedRootIds: context.followedRootIds,
    authoredRootIds: context.authoredRootIds,
    mutedRootIds: context.mutedRootIds,
    mutedChannelIds: context.mutedChannelIds,
    channelId: channel.id,
  });
  const mention = !isDm && hasMentionForEvent(event, pubkey);

  const { settings } = context;
  const allowed = (slot: "dm" | "mention" | "thread_reply") =>
    settings.desktopEnabled && settings.slotAlertsEnabled[slot];

  let notify: WatchedEventEffect["notify"] = null;
  if (isDm) {
    if (allowed("dm")) notify = "dm";
  } else if (mention) {
    if (allowed("mention")) notify = "mention";
  } else if (unread && isThreadReply(event.tags) && allowed("thread_reply")) {
    notify = "thread_reply";
  }

  return { notify, unread: unread || mention, mention };
}
