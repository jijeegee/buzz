import type {
  HomeFeedResponse,
  FeedItem,
  RelayEvent,
} from "@/shared/api/types";
import { getThreadReference } from "@/features/messages/lib/threading";
import {
  KIND_DELETION,
  KIND_NIP29_DELETE_EVENT,
} from "@/shared/constants/kinds";

/** Query key for deletions of inbox messages; invalidate when one is deleted. */
export const INBOX_DELETIONS_QUERY_KEY = ["inbox-deletions"] as const;

const HEX_EVENT_ID = /^[0-9a-f]{64}$/i;

function feedSections(feed: HomeFeedResponse["feed"]) {
  return [
    feed.mentions,
    feed.needsAction,
    feed.activity,
    feed.agentActivity,
  ] as const;
}

/**
 * Every event an inbox row depends on: the message itself plus its thread
 * root and parent. A row disappears when any of them is deleted, so a deleted
 * thread takes its replies' rows with it (the relay does not cascade).
 */
export function collectInboxReferencedEventIds(
  feed: HomeFeedResponse["feed"],
): string[] {
  const ids = new Set<string>();
  for (const section of feedSections(feed)) {
    for (const item of section) {
      ids.add(item.id);
      const thread = getThreadReference(item.tags);
      if (thread.rootId) ids.add(thread.rootId);
      if (thread.parentId) ids.add(thread.parentId);
    }
  }
  return [...ids].filter((id) => HEX_EVENT_ID.test(id)).sort();
}

/** Targets of deletion events (kind 5 and NIP-29 9005). */
export function getDeletedEventIds(events: readonly RelayEvent[]) {
  const deleted = new Set<string>();
  for (const event of events) {
    if (
      event.kind !== KIND_DELETION &&
      event.kind !== KIND_NIP29_DELETE_EVENT
    ) {
      continue;
    }
    for (const tag of event.tags) {
      if (tag[0] === "e" && typeof tag[1] === "string") deleted.add(tag[1]);
    }
  }
  return deleted;
}

function isDeletedFeedItem(item: FeedItem, deleted: ReadonlySet<string>) {
  if (deleted.has(item.id)) return true;
  const thread = getThreadReference(item.tags);
  return Boolean(
    (thread.rootId && deleted.has(thread.rootId)) ||
      (thread.parentId && deleted.has(thread.parentId)),
  );
}

/** Drops rows whose message, thread root, or parent was deleted. */
export function withoutDeletedFeedItems(
  feed: HomeFeedResponse,
  deleted: ReadonlySet<string>,
): HomeFeedResponse {
  if (deleted.size === 0) return feed;
  const keep = (items: FeedItem[]) =>
    items.filter((item) => !isDeletedFeedItem(item, deleted));
  return {
    ...feed,
    feed: {
      ...feed.feed,
      activity: keep(feed.feed.activity),
      agentActivity: keep(feed.feed.agentActivity),
      mentions: keep(feed.feed.mentions),
      needsAction: keep(feed.feed.needsAction),
    },
  };
}
