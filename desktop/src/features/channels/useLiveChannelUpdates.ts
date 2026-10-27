import * as React from "react";
import { type QueryClient, useQueryClient } from "@tanstack/react-query";

import { channelsQueryKey } from "@/features/channels/hooks";
import { INBOX_DELETIONS_QUERY_KEY } from "@/features/home/lib/inboxDeletions";
import {
  applyLiveEventToInboxSummary,
  deletedEventIds,
  INBOX_SUMMARY_QUERY_KEY,
  inboxSummaryShowsAny,
  type InboxSummaryState,
} from "@/features/home/lib/inboxSummary";
import { ChatListLiveSync } from "@/features/chats/chatListSync";
import { dmVisibilityQueryKey } from "@/features/channels/useHiddenDmIds";
import { updateChannelLastMessageAt } from "@/features/channels/lib/channelRecency";
import { mergeTimelineCacheMessages } from "@/features/messages/hooks";
import { useBlockedPeople } from "@/features/people/BlockedPeopleContext";
import { channelMessagesKey } from "@/features/messages/lib/messageQueryKeys";
import {
  getChannelIdFromTags,
  isThreadReply,
} from "@/features/messages/lib/threading";
import {
  hasMentionForEvent,
  shouldNotifyForEvent,
} from "@/features/notifications/lib/shouldNotify";
import { relayClient } from "@/shared/api/relayClient";
import {
  CHANNEL_MESSAGE_EVENT_KINDS,
  HOME_MENTION_EVENT_KINDS,
  KIND_DELETION,
  KIND_NIP29_DELETE_EVENT,
} from "@/shared/constants/kinds";
import type { Channel, RelayEvent } from "@/shared/api/types";
import {
  createTrailingDebounce,
  type TrailingDebounce,
} from "@/shared/lib/trailingDebounce";

import { isDmNotifiableKind } from "./isDmNotifiableKind";
import { refreshChannelsWhenIdle } from "./refreshChannelsWhenIdle";
import { isChatRoom } from "./lib/groupChat";

export type UseLiveChannelUpdatesOptions = {
  currentPubkey?: string;
  /**
   * When true, DM notifications also fire for the channel the user is
   * currently viewing (normally suppressed).
   */
  notifyForActiveChannel?: boolean;
  onDmMessage?: (event: RelayEvent, channel: Channel) => void;
  onLiveMention?: () => void;
  /**
   * Fired for live "new content" events in a member channel authored by
   * someone other than the current user. Thread replies also fire
   * onThreadReplyNotification so Home inbox activity stays in sync. Used to
   * drive the observed unread-event map that powers sidebar unread state.
   * See `UNREAD_TRIGGER_KINDS` for the exact kind set.
   */
  onChannelMessage?: (channelId: string, event: RelayEvent) => void;
  /**
   * Fired for thread replies that should be surfaced as Home inbox activity.
   */
  onThreadReplyNotification?: (channelId: string, event: RelayEvent) => void;
  /**
   * Fired for external thread replies that do not match the locally-known
   * interest sets. Callers can perform an async backfill and then decide
   * whether to surface the event.
   */
  onThreadReplyCandidate?: (channelId: string, event: RelayEvent) => void;
  /**
   * Fired for replies in threads the user authored, participated in, or
   * follows (non-DM channels only — the DM path owns those). Follows the DM
   * active-channel rule: suppressed for the channel being viewed unless
   * notifyForActiveChannel opts in.
   */
  onThreadReplyDesktopNotification?: (
    channelId: string,
    event: RelayEvent,
  ) => void;
  onSelfChannelMessage?: (event: RelayEvent) => void;
  participatedRootIds?: ReadonlySet<string>;
  followedRootIds?: ReadonlySet<string>;
  authoredRootIds?: ReadonlySet<string>;
  mutedRootIds?: ReadonlySet<string>;
  mutedChannelIds?: ReadonlySet<string>;
};

// get_channels is an expensive O(channels) relay fan-out. Incoming traffic for
// non-active channels arrives in bursts, so coalesce the refetch into a single
// trailing invalidation instead of one per event.
const CHANNELS_INVALIDATE_DEBOUNCE_MS = 500;

// Only "new content" kinds should bump unread state. Shared with the
// catch-up query in useUnreadChannels so the two paths stay in lockstep.
const UNREAD_TRIGGER_KINDS = new Set<number>(CHANNEL_MESSAGE_EVENT_KINDS);

export const EMPTY_SET: ReadonlySet<string> = new Set();

export function isChannelUnreadTriggerKind(kind: number, isDmChannel: boolean) {
  return isDmChannel
    ? isDmNotifiableKind(kind)
    : UNREAD_TRIGGER_KINDS.has(kind);
}

export function isHomeActivityEvent(
  isDmChannel: boolean,
  isThreadedReply: boolean,
) {
  return isThreadedReply || isDmChannel;
}

export function withChannelTagFallback(
  event: RelayEvent,
  channelId: string,
): RelayEvent {
  return getChannelIdFromTags(event.tags)
    ? event
    : { ...event, tags: [...event.tags, ["h", channelId]] };
}

function isExternalMentionEvent(event: RelayEvent, currentPubkey: string) {
  return (
    currentPubkey.length > 0 && event.pubkey.toLowerCase() !== currentPubkey
  );
}

/**
 * The room of a loaded event, for reactions and removals that carry no `h`
 * tag: looked up in the loaded timelines and threads.
 */
export function findRoomOfCachedEvent(
  queryClient: QueryClient,
  eventId: string,
): string | null {
  for (const queryKey of [["channel-messages"], ["thread-replies"]]) {
    for (const [key, events] of queryClient.getQueriesData<unknown>({
      queryKey,
    })) {
      if (
        typeof key[1] === "string" &&
        Array.isArray(events) &&
        events.some((event: RelayEvent) => event?.id === eventId)
      ) {
        return key[1];
      }
    }
  }
  return null;
}

const SEEN_NOTIFICATION_EVENT_LIMIT = 5_000;

export function trackSeenEvent(
  seenEventIds: Set<string>,
  eventId: string,
  limit = 200,
): boolean {
  if (seenEventIds.has(eventId)) {
    return false;
  }

  seenEventIds.add(eventId);
  if (seenEventIds.size > limit) {
    const oldestEventId = seenEventIds.values().next().value;
    if (oldestEventId) {
      seenEventIds.delete(oldestEventId);
    }
  }

  return true;
}

export function useLiveChannelUpdates(
  channels: Channel[],
  activeChannelId: string | null,
  options: UseLiveChannelUpdatesOptions = {},
) {
  const queryClient = useQueryClient();
  const { isBlocked } = useBlockedPeople();
  const normalizedCurrentPubkey =
    options.currentPubkey?.trim().toLowerCase() ?? "";
  const seenMentionEventIdsRef = React.useRef(new Set<string>());
  // Reconnect replay overlaps live filters so no message is lost at the
  // boundary. Guard notification side effects against repeated delivery.
  const seenNotificationEventIdsRef = React.useRef(new Set<string>());
  const channelsInvalidateRef = React.useRef<TrailingDebounce | null>(null);
  if (channelsInvalidateRef.current === null) {
    channelsInvalidateRef.current = createTrailingDebounce(() => {
      refreshChannelsWhenIdle({
        isFetching: () =>
          queryClient.isFetching({ queryKey: channelsQueryKey }),
        invalidate: () => {
          void queryClient.invalidateQueries({ queryKey: channelsQueryKey });
        },
        reArm: () => channelsInvalidateRef.current?.trigger(),
      });
    }, CHANNELS_INVALIDATE_DEBOUNCE_MS);
  }
  const invalidateChannelsDebounced = React.useCallback(() => {
    channelsInvalidateRef.current?.trigger();
  }, []);
  // Server chats list (Chats S4): live patches, and throttled refetches for
  // rows only the relay can place. Hidden DMs and archived rooms are skipped.
  const chatListSyncRef = React.useRef<ChatListLiveSync | null>(null);
  if (chatListSyncRef.current === null) {
    chatListSyncRef.current = new ChatListLiveSync(queryClient, {
      isRoomHidden: (channelId) =>
        queryClient
          .getQueriesData<unknown>({ queryKey: dmVisibilityQueryKey })
          .some(([, ids]) => ids instanceof Set && ids.has(channelId)) ||
        queryClient
          .getQueriesData<unknown>({ queryKey: channelsQueryKey })
          .some(
            ([, list]) =>
              Array.isArray(list) &&
              list.some(
                (channel: Channel) =>
                  channel.id === channelId && !!channel.archivedAt,
              ),
          ),
    });
  }
  React.useEffect(() => () => chatListSyncRef.current?.dispose(), []);
  const liveChannelIds = React.useMemo(
    () => new Set(channels.map((channel) => channel.id)),
    [channels],
  );
  // 1:1s and ownerless chats: alert and count unread on every message.
  const dmChannelMap = React.useMemo(
    () =>
      new Map(
        channels
          .filter((channel) => isChatRoom(channel))
          .map((channel) => [channel.id, channel]),
      ),
    [channels],
  );
  const dmSubscriptionStartedAtRef = React.useRef(0);

  // Reset subscription timestamp when identity changes.
  React.useEffect(() => {
    void normalizedCurrentPubkey;
    dmSubscriptionStartedAtRef.current = 0;
  }, [normalizedCurrentPubkey]);

  // Effect deps use primitive keys so refetches that produce new refs with
  // identical contents don't churn subscriptions. The Set/array memos are
  // still handy for closure reads via useEffectEvent.
  const channelIdsKey = React.useMemo(
    () => [...new Set(channels.map((channel) => channel.id))].sort().join(","),
    [channels],
  );

  const handleDmEvent = React.useEffectEvent(
    (event: RelayEvent, isFirstNotificationDelivery: boolean) => {
      // Only human-visible message kinds should fire DM notifications.
      if (!isDmNotifiableKind(event.kind) || !isFirstNotificationDelivery) {
        return;
      }

      // Suppress backlog events that predate our subscription — these are
      // historical replays, not live messages.
      if (event.created_at < dmSubscriptionStartedAtRef.current) {
        return;
      }

      const channelId = getChannelIdFromTags(event.tags);
      if (!channelId) {
        return;
      }

      if (!isExternalMentionEvent(event, normalizedCurrentPubkey)) {
        return;
      }

      const dmChannel = dmChannelMap.get(channelId);
      if (!dmChannel) {
        return;
      }

      // Muted rooms stay quiet. In a chat a mention of you still breaks
      // through; a 1:1 tags every participant, so it has no such signal.
      if (
        options.mutedChannelIds?.has(channelId) &&
        (dmChannel.channelType === "dm" ||
          !hasMentionForEvent(event, normalizedCurrentPubkey))
      ) {
        return;
      }

      // Don't fire a notification for the channel the user is already viewing,
      // unless the notify-while-viewing setting opts in.
      if (channelId === activeChannelId && !options.notifyForActiveChannel) {
        return;
      }

      options.onDmMessage?.(event, dmChannel);
    },
  );

  const handleIncomingMessage = React.useEffectEvent((event: RelayEvent) => {
    const channelId = getChannelIdFromTags(event.tags);
    if (!channelId) {
      return;
    }

    // A deleted message or thread must drop out of the inbox right away.
    if (
      event.kind === KIND_DELETION ||
      event.kind === KIND_NIP29_DELETE_EVENT
    ) {
      void queryClient.invalidateQueries({
        queryKey: INBOX_DELETIONS_QUERY_KEY,
      });
      // Reload the summary only when the deletion removes a message the chat
      // list shows; other deletions (e.g. removed reactions) change no row.
      const summary = queryClient.getQueryData<InboxSummaryState>(
        INBOX_SUMMARY_QUERY_KEY,
      );
      if (summary && inboxSummaryShowsAny(summary, deletedEventIds(event))) {
        void queryClient.invalidateQueries({
          queryKey: INBOX_SUMMARY_QUERY_KEY,
        });
      }
      chatListSyncRef.current?.onDeletion(deletedEventIds(event));
    }

    if (!liveChannelIds.has(channelId)) {
      if (channelId !== activeChannelId) {
        invalidateChannelsDebounced();
        // A room new to this device (e.g. a first DM) is new to the list too.
        chatListSyncRef.current?.refetchForMissingRow(channelId);
      }
      return;
    }

    const isDmChannel = dmChannelMap.has(channelId);
    // Someone I blocked: the message still lands in the timeline (folded
    // there), but never in the chat list, the inbox, a badge or an alert.
    const fromBlocked = isBlocked(event.pubkey);
    // Mention kinds are already in the channel stream. Keep Home's narrower
    // kind/recipient policy without opening a second REQ for every channel.
    if (
      !fromBlocked &&
      options.onLiveMention &&
      HOME_MENTION_EVENT_KINDS.some((kind) => kind === event.kind) &&
      isExternalMentionEvent(event, normalizedCurrentPubkey) &&
      hasMentionForEvent(event, normalizedCurrentPubkey) &&
      trackSeenEvent(seenMentionEventIdsRef.current, event.id)
    ) {
      options.onLiveMention();
    }
    const isUnreadTriggerKind = isChannelUnreadTriggerKind(
      event.kind,
      isDmChannel,
    );

    // Recency is presentation state, not notification state. Every recognized
    // message advances Recent ordering, including self-authored and muted
    // messages that the notification policy deliberately filters below.
    if (isUnreadTriggerKind) {
      updateChannelLastMessageAt(queryClient, channelId, event.created_at);
    }
    // The chat list changes only the row this message belongs to.
    if (!fromBlocked) {
      queryClient.setQueryData<InboxSummaryState>(
        INBOX_SUMMARY_QUERY_KEY,
        (current) =>
          current &&
          applyLiveEventToInboxSummary(
            current,
            event,
            {
              id: channelId,
              channelType: dmChannelMap.get(channelId)?.channelType ?? "stream",
            },
            normalizedCurrentPubkey,
          ),
      );
      chatListSyncRef.current?.onMessage(
        event,
        channelId,
        normalizedCurrentPubkey,
        hasMentionForEvent(event, normalizedCurrentPubkey),
      );
    }

    // Let the caller observe self-authored trigger events (e.g. to track
    // thread participation) before the author-exclusion guard filters them.
    if (
      isUnreadTriggerKind &&
      normalizedCurrentPubkey.length > 0 &&
      event.pubkey.toLowerCase() === normalizedCurrentPubkey
    ) {
      options.onSelfChannelMessage?.(event);
    }

    // Notify the unread tracker. Restricted to human-visible message kinds
    // and to events authored by someone other than the current user — your
    // own outgoing messages should never make a channel unread, and
    // reactions / edits / system messages aren't "new content".
    const isExternalTriggerEvent =
      !fromBlocked &&
      isUnreadTriggerKind &&
      (normalizedCurrentPubkey.length === 0 ||
        event.pubkey.toLowerCase() !== normalizedCurrentPubkey);
    const isFirstNotificationDelivery =
      !isExternalTriggerEvent ||
      trackSeenEvent(
        seenNotificationEventIdsRef.current,
        event.id,
        SEEN_NOTIFICATION_EVENT_LIMIT,
      );
    const isThreadedReply = isThreadReply(event.tags);

    // DM alerts and every other notification side effect share this delivery
    // decision, preventing a replayed event from escaping through a second
    // callback path.
    if (!fromBlocked) handleDmEvent(event, isFirstNotificationDelivery);

    if (isExternalTriggerEvent && isFirstNotificationDelivery) {
      const shouldNotify = shouldNotifyForEvent(
        event,
        normalizedCurrentPubkey,
        {
          participatedRootIds: options.participatedRootIds ?? EMPTY_SET,
          followedRootIds: options.followedRootIds ?? EMPTY_SET,
          authoredRootIds: options.authoredRootIds ?? EMPTY_SET,
          mutedRootIds: options.mutedRootIds ?? EMPTY_SET,
          mutedChannelIds: options.mutedChannelIds ?? EMPTY_SET,
          channelId,
        },
      );

      if (!shouldNotify) {
        if (isThreadedReply) {
          options.onThreadReplyCandidate?.(channelId, event);
        }
      } else {
        options.onChannelMessage?.(channelId, event);
        if (isHomeActivityEvent(isDmChannel, isThreadedReply)) {
          options.onThreadReplyNotification?.(channelId, event);
        }
      }

      if (shouldNotify && isThreadedReply) {
        if (
          !dmChannelMap.has(channelId) &&
          (channelId !== activeChannelId || options.notifyForActiveChannel)
        ) {
          options.onThreadReplyDesktopNotification?.(channelId, event);
        }
      }
    }

    // Merge into the timeline cache for the active channel.
    // useChannelSubscription also writes to this cache, but there's a
    // race window where it hasn't connected yet. Writes are idempotent
    // (mergeTimelineCacheMessages deduplicates by event ID).
    queryClient.setQueryData<RelayEvent[]>(
      channelMessagesKey(channelId),
      (current) => {
        if (!current) {
          return current;
        }

        return mergeTimelineCacheMessages(current, event);
      },
    );
  });

  React.useEffect(() => {
    return relayClient.subscribeToReconnects(() => {
      void queryClient.invalidateQueries({ queryKey: channelsQueryKey });
      // Messages missed while disconnected never reached the row patcher.
      void queryClient.invalidateQueries({ queryKey: INBOX_SUMMARY_QUERY_KEY });
      chatListSyncRef.current?.refetchNow();

      // Update the subscription timestamp so replayed backlog events
      // (which have created_at in the past) are naturally suppressed.
      dmSubscriptionStartedAtRef.current = Math.floor(Date.now() / 1000);
    });
  }, [queryClient]);

  React.useEffect(
    () =>
      relayClient.subscribeToChannelAccessRevocations(
        invalidateChannelsDebounced,
      ),
    [invalidateChannelsDebounced],
  );

  // Every member room's events arrive on the client's shared live room
  // bundle (one REQ per 128 rooms) instead of one REQ per room.
  React.useEffect(() => {
    const roomIds = channelIdsKey ? channelIdsKey.split(",") : [];
    if (roomIds.length > 0) {
      // Record the subscription start time so handleDmEvent can distinguish
      // backlog replays (created_at < startedAt) from live messages.
      dmSubscriptionStartedAtRef.current = Math.floor(Date.now() / 1000);
    }
    relayClient.liveRooms.setMemberRooms(roomIds);
    return () => relayClient.liveRooms.setMemberRooms([]);
  }, [channelIdsKey]);

  React.useEffect(
    () =>
      relayClient.liveRooms.listenAllRooms((event) => {
        handleIncomingMessage(event);
      }),
    [],
  );

  React.useEffect(
    () =>
      relayClient.liveRooms.setRoomResolver((eventId) =>
        findRoomOfCachedEvent(queryClient, eventId),
      ),
    [queryClient],
  );

  React.useEffect(() => {
    return () => {
      channelsInvalidateRef.current?.cancel();
    };
  }, []);
}
