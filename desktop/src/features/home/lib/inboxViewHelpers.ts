import {
  formatInboxFullTimestamp,
  type InboxContextMessage,
  type InboxFilter,
  type InboxItem,
} from "@/features/home/lib/inbox";
import { isProjectInboxItem } from "@/features/home/lib/projectInbox";
import {
  getChannelIdFromTags,
  getThreadReference,
  isBroadcastReply,
} from "@/features/messages/lib/threading";
import type { TimelineMessage } from "@/features/messages/types";
import type {
  FeedItem,
  RelayEvent,
  UserProfileSummary,
} from "@/shared/api/types";
import {
  KIND_FORUM_POST,
  KIND_REMINDER,
  KIND_STREAM_MESSAGE,
  KIND_STREAM_MESSAGE_V2,
} from "@/shared/constants/kinds";
import { normalizePubkey } from "@/shared/lib/pubkey";
import { resolveMentionProps } from "@/shared/lib/resolveMentionNames";

function hasThreadReplyTags(tags: string[][]) {
  const thread = getThreadReference(tags);
  return thread.parentId !== null && !isBroadcastReply(tags);
}

const CHANNEL_MESSAGE_KINDS = new Set([
  KIND_STREAM_MESSAGE,
  KIND_STREAM_MESSAGE_V2,
  KIND_FORUM_POST,
]);

function isChannelMainItem(item: FeedItem | undefined) {
  return Boolean(
    item?.channelId &&
      CHANNEL_MESSAGE_KINDS.has(item.kind) &&
      !isProjectInboxItem(item) &&
      !hasThreadReplyTags(item.tags),
  );
}

export function filterInboxItems(items: InboxItem[]) {
  return items.filter((item) => item.item.kind !== KIND_REMINDER);
}

export function hasInboxThreadContext(
  item: Pick<InboxItem, "groupItems" | "item">,
  contextMessages: readonly Pick<InboxContextMessage, "tags">[] = [],
) {
  return [item.item, ...item.groupItems, ...contextMessages].some((event) =>
    hasThreadReplyTags(event.tags ?? []),
  );
}

export function matchesInboxFilter(
  item: {
    categories: readonly string[];
    groupItems?: readonly FeedItem[];
    item?: FeedItem;
  },
  filter: InboxFilter,
  ownedAgentPubkeys?: ReadonlySet<string>,
) {
  if (filter === "all") {
    return ownedAgentPubkeys
      ? matchesInboxAllView(item, ownedAgentPubkeys)
      : true;
  }

  if (filter === "thread") {
    return [item.item, ...(item.groupItems ?? [])].some((groupItem) =>
      groupItem ? hasThreadReplyTags(groupItem.tags) : false,
    );
  }

  if (filter === "conversations") {
    const groupItems = [item.item, ...(item.groupItems ?? [])];
    return (
      groupItems.some((groupItem) =>
        groupItem ? hasThreadReplyTags(groupItem.tags) : false,
      ) ||
      groupItems.every(
        (groupItem) => !groupItem || isChannelMainItem(groupItem),
      )
    );
  }

  if (filter === "project") {
    return [item.item, ...(item.groupItems ?? [])].some(
      (groupItem) => groupItem && isProjectInboxItem(groupItem),
    );
  }

  if (filter === "agent_activity" && ownedAgentPubkeys) {
    const representative = item.item ?? item.groupItems?.at(-1);
    return representative
      ? ownedAgentPubkeys.has(normalizePubkey(representative.pubkey))
      : false;
  }

  return item.categories.includes(filter);
}

const CHANNEL_MAIN_ROW_PREFIX = "channel:";

/** True for a row that stands for a whole channel main timeline. */
export function isChannelMainRoomItem(item: InboxItem | null | undefined) {
  return item?.conversationId.startsWith(CHANNEL_MAIN_ROW_PREFIX) === true;
}

/**
 * The "conversations" view treats each channel's main timeline like one
 * thread: top-level rows from the same channel collapse into a single row
 * keyed `channel:<id>`, ordered by latest activity alongside thread rows.
 * DM rows are already grouped per channel and pass through unchanged.
 */
export function collapseChannelMainRows(items: readonly InboxItem[]) {
  const rows: InboxItem[] = [];
  const channelRows = new Map<string, InboxItem>();

  for (const item of items) {
    const channelId = item.item.channelId;
    const isMainRow =
      channelId &&
      item.item.channelType !== "dm" &&
      [item.item, ...item.groupItems].every(isChannelMainItem);
    if (!channelId || !isMainRow) {
      rows.push(item);
      continue;
    }

    const existing = channelRows.get(channelId);
    if (!existing) {
      const row = {
        ...item,
        conversationId: `${CHANNEL_MAIN_ROW_PREFIX}${channelId}`,
        groupItems: [...item.groupItems],
      };
      channelRows.set(channelId, row);
      rows.push(row);
      continue;
    }

    const [latest, earlier] =
      item.latestActivityAt > existing.latestActivityAt
        ? [item, existing]
        : [existing, item];
    const merged: InboxItem = {
      ...latest,
      conversationId: existing.conversationId,
      categories: [...new Set([...latest.categories, ...earlier.categories])],
      groupItems: [...existing.groupItems, ...item.groupItems],
      isActionRequired: existing.isActionRequired || item.isActionRequired,
      latestActivityAt: Math.max(
        existing.latestActivityAt,
        item.latestActivityAt,
      ),
      unreadCount: existing.unreadCount + item.unreadCount,
    };
    channelRows.set(channelId, merged);
    rows[rows.indexOf(existing)] = merged;
  }

  return rows.sort(
    (left, right) => right.latestActivityAt - left.latestActivityAt,
  );
}

export function matchesInboxAllView(
  item: {
    categories: readonly string[];
    groupItems?: readonly FeedItem[];
    item?: FeedItem;
  },
  ownedAgentPubkeys: ReadonlySet<string>,
): boolean {
  const representative = item.item ?? item.groupItems?.at(-1);
  return (
    representative?.channelType === "dm" ||
    item.categories.includes("mention") ||
    [item.item, ...(item.groupItems ?? [])].some((groupItem) =>
      groupItem ? hasThreadReplyTags(groupItem.tags) : false,
    ) ||
    [item.item, ...(item.groupItems ?? [])].some(
      (groupItem) => groupItem && isProjectInboxItem(groupItem),
    ) ||
    item.categories.includes("needs_action") ||
    Boolean(
      representative &&
        ownedAgentPubkeys.has(normalizePubkey(representative.pubkey)),
    )
  );
}

export function getContextMessageDepth(
  event: RelayEvent,
  eventById: ReadonlyMap<string, RelayEvent>,
): number {
  let depth = 0;
  let parentId = getThreadReference(event.tags).parentId;
  const seen = new Set<string>([event.id]);

  while (parentId && eventById.has(parentId) && !seen.has(parentId)) {
    depth += 1;
    seen.add(parentId);
    parentId = getThreadReference(eventById.get(parentId)?.tags ?? []).parentId;
  }

  return depth;
}

export function isInboxThreadContextEvent(
  event: RelayEvent,
  selection: {
    selectedChannelId: string | null;
    selectedEventId: string;
    selectedParentId: string | null;
    selectedThreadRootId: string | null;
  },
): boolean {
  if (
    selection.selectedChannelId &&
    getChannelIdFromTags(event.tags) !== selection.selectedChannelId
  ) {
    return false;
  }

  if (event.id === selection.selectedEventId) {
    return true;
  }

  if (
    selection.selectedThreadRootId &&
    event.id === selection.selectedThreadRootId
  ) {
    return true;
  }

  if (selection.selectedParentId && event.id === selection.selectedParentId) {
    return true;
  }

  const thread = getThreadReference(event.tags);
  return (
    (selection.selectedThreadRootId !== null &&
      (thread.rootId === selection.selectedThreadRootId ||
        thread.parentId === selection.selectedThreadRootId)) ||
    thread.parentId === selection.selectedEventId
  );
}

export function getReactionTargetId(tags: string[][]) {
  for (let index = tags.length - 1; index >= 0; index -= 1) {
    const tag = tags[index];
    if (tag?.[0] === "e" && typeof tag[1] === "string") {
      return tag[1];
    }
  }

  return null;
}

/**
 * Maps a formatted timeline message into the inbox detail pane's context
 * message shape. Extracted from HomeView's `contextMessages` memo so the
 * field selection is unit-testable: `kind` and `signerPubkey` must survive
 * this mapping for the config-nudge trust gate
 * (`getConfigNudgeAuthorPubkey`) to run in `InboxMessageRow` — an earlier
 * version dropped them, structurally disabling the card on the inbox
 * surface.
 */
export function toInboxContextMessage(
  message: TimelineMessage,
  context: {
    eventById: ReadonlyMap<string, RelayEvent>;
    fallbackAuthorPubkey: string;
    profiles: Record<string, UserProfileSummary> | undefined;
    selectedItemId: string;
  },
): InboxContextMessage {
  const event = context.eventById.get(message.id);
  const authorPubkey =
    message.pubkey ?? event?.pubkey ?? context.fallbackAuthorPubkey;
  const { mentionNames, mentionPubkeysByName } = resolveMentionProps(
    message.tags ?? [],
    context.profiles,
    message.body,
  );
  return {
    id: message.id,
    authorLabel: message.author,
    authorPubkey,
    isAgent: message.isAgent,
    ownerLabel: message.ownerLabel,
    ownerPubkey: message.ownerPubkey,
    avatarUrl: message.avatarUrl ?? null,
    content: message.body,
    createdAt: message.createdAt,
    depth: event
      ? getContextMessageDepth(event, context.eventById)
      : message.depth,
    fullTimestampLabel: formatInboxFullTimestamp(message.createdAt),
    isSelected: message.id === context.selectedItemId,
    kind: message.kind,
    mentionNames: mentionNames ?? [],
    mentionPubkeysByName,
    reactions: message.reactions,
    signerPubkey: message.signerPubkey,
    tags: message.tags,
    timeLabel: message.time,
  };
}

/**
 * Converts an inbox context message back into the `TimelineMessage` shape
 * the shared message components consume (action bar, reactions, and the
 * config-nudge gate — `kind` and `signerPubkey` ride through for
 * `getConfigNudgeAuthorPubkey`).
 */
export function toTimelineMessage(
  message: InboxContextMessage,
): TimelineMessage {
  const threadReference = getThreadReference(message.tags ?? []);
  return {
    id: message.id,
    author: message.authorLabel,
    isAgent: message.isAgent,
    ownerLabel: message.ownerLabel,
    ownerPubkey: message.ownerPubkey,
    avatarUrl: message.avatarUrl,
    body: message.content,
    createdAt: message.createdAt,
    depth: message.depth,
    kind: message.kind,
    parentId: message.parentId ?? threadReference.parentId,
    pubkey: message.authorPubkey,
    reactions: message.reactions ?? [],
    rootId: message.rootId ?? threadReference.rootId,
    signerPubkey: message.signerPubkey,
    tags: message.tags,
    time: message.timeLabel ?? message.fullTimestampLabel,
  };
}
