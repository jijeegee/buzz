import type { FeedItem, HomeFeedResponse } from "@/shared/api/types";

/**
 * The home feed without anything written by someone I blocked, so their
 * mentions and activity never reach the Inbox, a badge or an alert. Returns
 * `response` itself when nothing is removed.
 */
export function withoutBlockedFeedItems(
  response: HomeFeedResponse,
  isBlocked: (pubkey: string) => boolean,
): HomeFeedResponse {
  const keep = (items: FeedItem[]) => {
    const kept = items.filter((item) => !isBlocked(item.pubkey));
    return kept.length === items.length ? items : kept;
  };
  const { feed } = response;
  const next = {
    mentions: keep(feed.mentions),
    needsAction: keep(feed.needsAction),
    activity: keep(feed.activity),
    agentActivity: keep(feed.agentActivity),
  };
  const unchanged =
    next.mentions === feed.mentions &&
    next.needsAction === feed.needsAction &&
    next.activity === feed.activity &&
    next.agentActivity === feed.agentActivity;
  return unchanged ? response : { ...response, feed: { ...feed, ...next } };
}
