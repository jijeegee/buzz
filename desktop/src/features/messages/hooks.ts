import { t } from "@/shared/i18n";
import { CONVERSATION_CACHE_GC_MS } from "@/shared/api/queryClient";
import {
  useCallback,
  useEffect,
  useEffectEvent,
  useRef,
  useState,
} from "react";
import {
  type QueryClient,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { toast } from "sonner";

import {
  channelMessagesKey,
  channelWindowKey,
  threadRepliesKey,
} from "@/features/messages/lib/messageQueryKeys";
import {
  buildReplyTags,
  getThreadReference,
  isBroadcastReply,
  normalizeMentionPubkeys,
  resolveReplyRootId,
} from "@/features/messages/lib/threading";
import {
  projectChannelWindowMessages,
  refreshChannelWindowMessages,
} from "@/features/messages/lib/projectChannelWindow";
import { reconcileChannelWindowMessages } from "@/features/messages/lib/channelWindowReconciliation";
import {
  channelHeadCacheScope,
  channelHeadHydration,
  consumeHydratedChannel,
} from "@/features/messages/lib/channelHeadCache";
import { storeChannelHeadCache } from "@/shared/api/tauriChannelHeadCache";
import {
  isMatchingPendingMessage,
  mergeMessages,
  mergeTimelineCacheMessages,
} from "@/features/messages/lib/messageMerge";

export { mergeMessages, mergeTimelineCacheMessages };
import { splitOutgoingTags } from "@/features/messages/lib/imetaMediaMarkdown";
import { messageMentionPubkeys } from "@/features/messages/lib/messageMentionPubkeys";
import { buildSentFromThreadTag } from "@/features/messages/lib/sentFromThread";
import {
  clearTimeoutState,
  recordTimeoutFromRejection,
} from "@/features/moderation/lib/timeoutStore";
import { relayClient, setVisibleChannel } from "@/shared/api/relayClient";
import { customEmojiQueryKey } from "@/features/custom-emoji/hooks";
import { channelsQueryKey } from "@/features/channels/hooks";
import { reactionEmojiUrl } from "@/shared/api/customEmoji";
import type { CustomEmoji } from "@/shared/lib/remarkCustomEmoji";
import {
  addReaction,
  deleteMessage,
  editMessage,
  removeReaction,
  sendChannelMessage,
} from "@/shared/api/tauri";
import { getChannelWindowEvents } from "@/shared/api/channelWindow";
import type { Channel, Identity, RelayEvent } from "@/shared/api/types";
// Same .mjs the renderer uses, so the cache-update projection can't drift
// from the on-render overlay.
import {
  createSettleTimers,
  refreshOnSystemMessage,
} from "@/features/channels/membershipSystemRefresh";
import { applyEditTagOverlay } from "@/features/messages/lib/applyEditTagOverlay.mjs";
import { cancelAutoRoute } from "@/features/messages/lib/autoRouteStatus";
import {
  hasFailedSend,
  registerFailedSend,
} from "@/features/messages/lib/failedSends";
import {
  type StoredSend,
  isStoredSendInFlight,
  markStoredSendInFlight,
  removeStoredSend,
  saveStoredSend,
  settleStoredSend,
  storedSends,
} from "@/features/messages/lib/failedSendStore";
import {
  emptyChannelWindowStore,
  flattenChannelWindowEvents,
  mapChannelWindowEvents,
  mergeLiveChannelWindowEvent,
  mergeLiveThreadSummary,
  replaceNewestChannelWindow,
  type ChannelWindowStore,
} from "@/features/messages/lib/channelWindowStore";
import {
  parseChannelWindowResponse,
  parseLiveThreadSummary,
} from "@/features/messages/lib/channelWindowResponse";
import {
  CHANNEL_AUX_EVENT_KINDS,
  CHANNEL_TIMELINE_CONTENT_KINDS,
  KIND_CHANNEL_THREAD_SUMMARY,
  KIND_STREAM_MESSAGE,
  KIND_SYSTEM_MESSAGE,
} from "@/shared/constants/kinds";
import { INBOX_DELETIONS_QUERY_KEY } from "@/features/home/lib/inboxDeletions";

export type SendMessageVariables = {
  channelId?: string;
  targetChannel?: Channel;
  content: string;
  mentionPubkeys?: string[];
  parentEventId?: string | null;
  mediaTags?: string[][];
  forceRest?: boolean;
  sentFromThreadRootId?: string | null;
  sentFromThreadRootExcerpt?: string | null;
  transport?: "auto" | "http";
  /**
   * Reuse the row a send preview already drew. Such a send keeps its row on
   * failure, marked failed with Retry / Delete, instead of rolling back.
   */
  optimisticLocalKey?: string;
};

type MessageQueryContext = {
  optimisticId: string;
  previousMessages: RelayEvent[];
  previousWindow: ChannelWindowStore | undefined;
  channelId: string;
  queryKey: ReturnType<typeof channelMessagesKey>;
};

const CHANNEL_TIMELINE_KINDS = new Set<number>(CHANNEL_TIMELINE_CONTENT_KINDS);
const CHANNEL_AUX_KINDS = new Set<number>(CHANNEL_AUX_EVENT_KINDS);

export function resolveCachedReplyRootId(
  parentEventId: string,
  messageCaches: readonly RelayEvent[][],
): string | null {
  for (const messages of messageCaches) {
    if (messages.some((event) => event.id === parentEventId)) {
      return resolveReplyRootId(parentEventId, messages);
    }
  }
  return null;
}

export function createOptimisticMessage(
  channelId: string,
  content: string,
  identity: Identity,
  currentMessages: RelayEvent[],
  mentionPubkeys: string[] = [],
  parentEventId: string | null = null,
  mediaTags: string[][] = [],
  sentFromThreadRootId: string | null = null,
  sentFromThreadRootExcerpt: string | null = null,
  localKey = `optimistic-${crypto.randomUUID()}`,
): RelayEvent {
  const tags: string[][] = [];

  if (parentEventId) {
    tags.push(
      ...buildReplyTags(
        channelId,
        identity.pubkey,
        parentEventId,
        resolveReplyRootId(parentEventId, currentMessages),
        mentionPubkeys,
      ),
    );
  } else {
    tags.push(["h", channelId]);
    tags.push(["p", identity.pubkey]);
    for (const pubkey of normalizeMentionPubkeys(
      mentionPubkeys,
      identity.pubkey,
    )) {
      tags.push(["p", pubkey]);
    }
  }

  for (const tag of mediaTags) {
    tags.push(tag);
  }
  if (sentFromThreadRootId) {
    tags.push(
      buildSentFromThreadTag(sentFromThreadRootId, sentFromThreadRootExcerpt),
    );
  }

  return {
    id: localKey,
    localKey,
    pubkey: identity.pubkey,
    created_at: Math.floor(Date.now() / 1_000),
    kind: KIND_STREAM_MESSAGE,
    tags,
    content,
    sig: "",
    pending: true,
  };
}

function channelThreadCaches(
  queryClient: QueryClient,
  channelId: string,
): RelayEvent[][] {
  return queryClient
    .getQueriesData<RelayEvent[]>({ queryKey: ["thread-replies", channelId] })
    .flatMap(([, events]) => (events ? [events] : []));
}

/**
 * Put an own optimistic row (or its replacement with the same id) into the
 * channel window and, for a reply, into its thread's cache when that thread
 * is loaded. Leaves an unloaded thread alone so its first fetch still runs.
 */
export function writeOptimisticMessage(
  queryClient: QueryClient,
  channelId: string,
  event: RelayEvent,
) {
  const windowKey = channelWindowKey(channelId);
  queryClient.setQueryData(
    windowKey,
    mergeLiveChannelWindowEvent(
      queryClient.getQueryData<ChannelWindowStore>(windowKey) ??
        emptyChannelWindowStore(),
      event,
    ),
  );
  projectChannelWindowMessages(queryClient, channelId);
  const { parentId, rootId } = getThreadReference(event.tags);
  if (!parentId || !rootId) return;
  const repliesKey = threadRepliesKey(channelId, rootId);
  if (queryClient.getQueryData(repliesKey) === undefined) return;
  queryClient.setQueryData<RelayEvent[]>(repliesKey, (current = []) =>
    mergeMessages(current, event),
  );
}

export function updateOptimisticMessage(
  queryClient: QueryClient,
  channelId: string,
  localKey: string,
  update: (event: RelayEvent) => RelayEvent | null,
) {
  const matches = (event: RelayEvent) =>
    event.pending === true && (event.localKey ?? event.id) === localKey;
  const apply = (events: RelayEvent[]) =>
    events.flatMap((event) => {
      if (!matches(event)) return [event];
      const next = update(event);
      return next ? [next] : [];
    });
  const windowKey = channelWindowKey(channelId);
  const store = queryClient.getQueryData<ChannelWindowStore>(windowKey);
  if (store?.liveOverlay.some(matches)) {
    queryClient.setQueryData(windowKey, {
      ...store,
      liveOverlay: apply(store.liveOverlay),
    });
  }
  // The rendered timeline retains pending rows the window lacks, so it must
  // change too or projection would bring the old row back.
  const messagesKey = channelMessagesKey(channelId);
  const messages = queryClient.getQueryData<RelayEvent[]>(messagesKey);
  if (messages?.some(matches)) {
    queryClient.setQueryData(messagesKey, apply(messages));
  }
  projectChannelWindowMessages(queryClient, channelId);
  for (const [key, events] of queryClient.getQueriesData<RelayEvent[]>({
    queryKey: ["thread-replies", channelId],
  })) {
    if (events?.some(matches)) queryClient.setQueryData(key, apply(events));
  }
}

function findOptimisticMessage(
  queryClient: QueryClient,
  channelId: string,
  localKey: string,
): RelayEvent | undefined {
  return [
    queryClient.getQueryData<RelayEvent[]>(channelMessagesKey(channelId)) ?? [],
    ...channelThreadCaches(queryClient, channelId),
  ]
    .flat()
    .find((event) => event.pending === true && event.id === localKey);
}

/** Whether the relay's copy of an own optimistic row has already arrived. */
function hasOwnEcho(
  queryClient: QueryClient,
  channelId: string,
  localKey: string,
): boolean {
  const row = findOptimisticMessage(queryClient, channelId, localKey);
  if (!row) return false;
  const store = queryClient.getQueryData<ChannelWindowStore>(
    channelWindowKey(channelId),
  );
  return [
    store ? flattenChannelWindowEvents(store) : [],
    queryClient.getQueryData<RelayEvent[]>(channelMessagesKey(channelId)) ?? [],
    ...channelThreadCaches(queryClient, channelId),
  ]
    .flat()
    .some(
      (event) =>
        event.created_at >= row.created_at &&
        isMatchingPendingMessage(row, event),
    );
}

/** Drop an own optimistic row that will not be sent. */
export function removeOptimisticMessage(
  queryClient: QueryClient,
  channelId: string,
  localKey: string,
) {
  updateOptimisticMessage(queryClient, channelId, localKey, () => null);
}

/** How far apart a stored send and a relay copy of it can be signed. */
const STORED_SEND_MATCH_WINDOW_SECONDS = 120;

/**
 * Asks the relay whether a stored send landed after all (the app quit
 * between the relay storing it and its acknowledgement). Until it answers,
 * the send is not drawn, so a delivered message never shows as failed and
 * Retry can never post it twice.
 */
export type StoredSendRelayCheck = {
  state: Map<string, "checking" | "absent">;
  lookup: (stored: StoredSend) => Promise<RelayEvent[]>;
  onSettled: () => void;
};

function isStoredSendCopy(stored: StoredSend, event: RelayEvent): boolean {
  const row: RelayEvent = { ...stored.event, pending: true };
  return (
    !event.pending &&
    Math.abs(event.created_at - row.created_at) <=
      STORED_SEND_MATCH_WINDOW_SECONDS &&
    isMatchingPendingMessage(row, event)
  );
}

/** Own messages in this channel signed around the stored send's time. */
export function lookupStoredSendOnRelay(
  stored: StoredSend,
): Promise<RelayEvent[]> {
  return relayClient.fetchEvents({
    kinds: [stored.event.kind],
    authors: [stored.event.pubkey],
    "#h": [stored.channelId],
    since: stored.event.created_at - STORED_SEND_MATCH_WINDOW_SECONDS,
    until: stored.event.created_at + STORED_SEND_MATCH_WINDOW_SECONDS,
    limit: 50,
  });
}

/**
 * Put back own sends the relay never acknowledged (from an earlier app
 * session, or from before a community switch): marked failed, with Retry /
 * Delete, where they were drawn. A stored send whose relay copy is here was
 * delivered; it is dropped. `attempted` keeps each write to once per cache,
 * so cache updates this causes cannot loop.
 */
export function restoreStoredSends(
  queryClient: QueryClient,
  channelId: string,
  retry: (variables: SendMessageVariables) => Promise<unknown>,
  attempted: Set<string>,
  relayCheck?: StoredSendRelayCheck,
) {
  const messages = queryClient.getQueryData<RelayEvent[]>(
    channelMessagesKey(channelId),
  );
  if (!messages) return;
  for (const stored of storedSends(channelId)) {
    const { localKey, rootId } = stored;
    const row: RelayEvent = { ...stored.event, pending: true, failed: true };
    const loaded = [messages, ...channelThreadCaches(queryClient, channelId)];
    const delivered = loaded
      .flat()
      .some(
        (event) =>
          (!event.pending && event.localKey === localKey) ||
          isStoredSendCopy(stored, event),
      );
    if (delivered) {
      // Also covers a late echo adopting a failed row in this session; once
      // adopted, an edit or delete of the message must not revive it.
      removeStoredSend(localKey);
      continue;
    }
    if (isStoredSendInFlight(localKey)) continue;
    const isShown = (events: RelayEvent[] | undefined) =>
      events?.some((event) => (event.localKey ?? event.id) === localKey) ??
      false;
    if (relayCheck && !isShown(messages)) {
      const checked = relayCheck.state.get(localKey);
      if (checked === undefined) {
        relayCheck.state.set(localKey, "checking");
        relayCheck
          .lookup(stored)
          .then((events) => {
            if (events.some((event) => isStoredSendCopy(stored, event))) {
              removeStoredSend(localKey);
            }
          })
          // Unreachable relay: show it failed so Retry / Delete stay possible.
          .catch(() => {})
          .finally(() => {
            relayCheck.state.set(localKey, "absent");
            relayCheck.onSettled();
          });
      }
      if (checked !== "absent") continue;
    }
    if (!attempted.has(`timeline:${localKey}`)) {
      attempted.add(`timeline:${localKey}`);
      if (!isShown(messages)) {
        writeOptimisticMessage(queryClient, channelId, row);
        if (
          !isShown(
            queryClient.getQueryData<RelayEvent[]>(
              channelMessagesKey(channelId),
            ),
          )
        ) {
          // Older than the loaded window: the timeline drops it there, so
          // draw it at the bottom rather than lose the text.
          writeOptimisticMessage(queryClient, channelId, {
            ...row,
            created_at: Math.floor(Date.now() / 1000),
          });
        }
      }
    }
    const repliesKey = rootId ? threadRepliesKey(channelId, rootId) : null;
    const replies = repliesKey
      ? queryClient.getQueryData<RelayEvent[]>(repliesKey)
      : undefined;
    if (repliesKey && replies && !attempted.has(`thread:${localKey}`)) {
      attempted.add(`thread:${localKey}`);
      if (!isShown(replies)) {
        queryClient.setQueryData(repliesKey, mergeMessages(replies, row));
      }
    }
    if (!hasFailedSend(localKey)) {
      registerFailedSend(localKey, {
        retry: () => {
          // In flight before the registry entry goes, so a cache update in
          // between cannot arm a second Retry.
          markStoredSendInFlight(localKey);
          retry({
            ...stored.send,
            channelId,
            optimisticLocalKey: localKey,
          }).catch(() => {});
        },
        discard: () => {
          removeOptimisticMessage(queryClient, channelId, localKey);
          removeStoredSend(localKey);
        },
      });
    }
  }
}

/** Restore stored own sends into a channel as its timeline and threads load. */
export function useRestoreStoredSends(
  channelId: string | null,
  retry: (variables: SendMessageVariables) => Promise<unknown>,
) {
  const queryClient = useQueryClient();
  const retryRef = useRef(retry);
  retryRef.current = retry;
  useEffect(() => {
    if (!channelId) return;
    const attempted = new Set<string>();
    let disposed = false;
    const relayCheck: StoredSendRelayCheck = {
      state: new Map(),
      lookup: lookupStoredSendOnRelay,
      onSettled: () => {
        if (!disposed) sync();
      },
    };
    let syncing = false;
    const sync = () => {
      if (syncing) return;
      syncing = true;
      try {
        restoreStoredSends(
          queryClient,
          channelId,
          (variables) => retryRef.current(variables),
          attempted,
          relayCheck,
        );
      } finally {
        syncing = false;
      }
    };
    sync();
    const unsubscribe = queryClient.getQueryCache().subscribe((event) => {
      const [scope, eventChannelId] = event.query.queryKey as unknown[];
      if (
        eventChannelId === channelId &&
        (scope === "channel-messages" || scope === "thread-replies")
      ) {
        sync();
      }
    });
    return () => {
      disposed = true;
      unsubscribe();
    };
  }, [channelId, queryClient]);
}

/** What a composer knows about a message at the moment the user sends it. */
export type SendPreviewInput = {
  channelId: string | null;
  content: string;
  mentionPubkeys: string[];
  mediaTags: string[][];
  parentEventId: string | null;
};

export type SendPreview = (
  input: SendPreviewInput,
) => { localKey: string; discard: () => void } | null;

/**
 * Draw the user's own message the moment they press send, before any
 * preparation or relay round trip. The send later reuses the row through
 * `optimisticLocalKey`; `discard` removes it when the send is abandoned.
 */
export function useSendPreview(
  channel: Channel | null,
  identity: Identity | undefined,
): SendPreview {
  const queryClient = useQueryClient();
  return useCallback(
    ({ channelId, content, mentionPubkeys, mediaTags, parentEventId }) => {
      const target = resolveEffectiveChannel(
        channelId,
        queryClient.getQueryData<Channel[]>(channelsQueryKey),
        channel,
      );
      if (!target || !identity || target.channelType === "forum") return null;
      const event = createOptimisticMessage(
        target.id,
        content.trim(),
        identity,
        [
          ...(queryClient.getQueryData<RelayEvent[]>(
            channelMessagesKey(target.id),
          ) ?? []),
          ...channelThreadCaches(queryClient, target.id).flat(),
        ],
        mentionPubkeys,
        parentEventId,
        mediaTags,
      );
      writeOptimisticMessage(queryClient, target.id, event);
      const localKey = event.id;
      return {
        localKey,
        discard: () =>
          removeOptimisticMessage(queryClient, target.id, localKey),
      };
    },
    [channel, identity, queryClient],
  );
}

/**
 * Resolves the effective target channel for a send operation.
 *
 * When `capturedChannelId` is supplied (non-null), the target is looked up from
 * `channelsCache` — this pins the send to the compose-time channel regardless
 * of any subsequent navigation. If the id is supplied but resolves to nothing,
 * returns `null` (caller should throw — don't silently fall back to the live
 * channel). When `capturedChannelId` is null, the caller didn't capture one and
 * the closed-over `fallbackChannel` is the intended target.
 *
 * Exported for unit testing.
 */
export function resolveEffectiveChannel(
  capturedChannelId: string | null | undefined,
  channelsCache: Channel[] | undefined,
  fallbackChannel: Channel | null,
): Channel | null {
  if (capturedChannelId == null) {
    return fallbackChannel;
  }
  return channelsCache?.find((c) => c.id === capturedChannelId) ?? null;
}

/**
 * Resolves a send target captured as either the channel object itself or its id.
 * A relay-returned channel remains authoritative even when the shared channel
 * list is temporarily stale and does not contain it.
 *
 * Exported for unit testing.
 */
export function resolveSendChannel(
  targetChannel: Channel | undefined,
  capturedChannelId: string | null | undefined,
  channelsCache: Channel[] | undefined,
  fallbackChannel: Channel | null,
): Channel | null {
  return (
    targetChannel ??
    resolveEffectiveChannel(capturedChannelId, channelsCache, fallbackChannel)
  );
}

/**
 * Resolves the thread reply target from a submit-time captured context or,
 * for callers that predate the capture pattern, from live refs.
 *
 * When `threadContext` is supplied (non-null), its values are used exclusively
 * — no live-ref reads occur. This is the race-free path: the context was
 * captured synchronously at submit time before any async awaits.
 *
 * When `threadContext` is null/undefined (legacy callers), falls back to
 * `liveReplyTargetId ?? liveThreadHeadId`.
 *
 * Returns null when no parentEventId can be resolved (caller should bail).
 */
export function resolveThreadReplyTarget(
  threadContext:
    | { parentEventId: string | null; threadHeadId: string | null }
    | null
    | undefined,
  liveReplyTargetId: string | null | undefined,
  liveThreadHeadId: string | null | undefined,
): { parentEventId: string; threadHeadId: string | null } | null {
  if (threadContext != null) {
    // Captured context: use exclusively — no ?? fallback to live refs.
    if (!threadContext.parentEventId) {
      return null;
    }
    return {
      parentEventId: threadContext.parentEventId,
      threadHeadId: threadContext.threadHeadId,
    };
  }
  // Legacy path: read from live refs.
  const parentEventId = liveReplyTargetId ?? liveThreadHeadId ?? null;
  if (!parentEventId) {
    return null;
  }
  return {
    parentEventId,
    threadHeadId: liveThreadHeadId ?? null,
  };
}

export function useChannelWindowQuery(channel: Channel | null) {
  const queryClient = useQueryClient();
  const queryKey = channelWindowKey(channel?.id ?? "none");
  return useQuery({
    enabled: channel !== null && channel.channelType !== "forum",
    queryKey,
    queryFn: () =>
      queryClient.getQueryData<ChannelWindowStore>(queryKey) ??
      emptyChannelWindowStore(),
    staleTime: Number.POSITIVE_INFINITY,
    gcTime: CONVERSATION_CACHE_GC_MS,
  });
}

export function reconcileFetchedChannelWindow(
  queryClient: QueryClient,
  channelId: string,
  events: Awaited<ReturnType<typeof getChannelWindowEvents>>,
  previousMessages: RelayEvent[],
  signal: AbortSignal,
  // Background loads must not rank as visits in the persisted-head LRU.
  persistHead = true,
): RelayEvent[] {
  // Tauri invokes cannot be canceled after dispatch. A replacement refetch can
  // therefore win while this older request is still in flight. Never let that
  // canceled request commit its stale page into the authoritative window.
  signal.throwIfAborted();
  const windowKey = channelWindowKey(channelId);
  const page = parseChannelWindowResponse(events, channelId, null);
  const current =
    queryClient.getQueryData<ChannelWindowStore>(windowKey) ??
    emptyChannelWindowStore();
  const next = replaceNewestChannelWindow(current, page);
  queryClient.setQueryData(windowKey, next);
  const scope = persistHead ? channelHeadCacheScope(queryClient) : null;
  if (scope) {
    void storeChannelHeadCache(scope, channelId, events).catch((error) => {
      console.warn("Failed to persist channel head", channelId, error);
    });
  }
  return reconcileChannelWindowMessages(next, previousMessages);
}

export function useChannelMessagesQuery(channel: Channel | null) {
  const queryClient = useQueryClient();
  const queryKey = channelMessagesKey(channel?.id ?? "none");
  return useQuery({
    enabled: channel !== null && channel.channelType !== "forum",
    queryKey,
    queryFn: async ({ signal }) => {
      if (!channel) throw new Error("No group selected.");
      // Persisted heads seed asynchronously; wait for that seed so a channel
      // opened during boot takes the hydrated path instead of racing it with
      // a cold relay fetch.
      await channelHeadHydration(queryClient);
      if (consumeHydratedChannel(queryClient, channel.id)) {
        return queryClient.getQueryData<RelayEvent[]>(queryKey) ?? [];
      }
      const previousMessages =
        queryClient.getQueryData<RelayEvent[]>(queryKey) ?? [];
      const events = await getChannelWindowEvents(channel.id);
      return reconcileFetchedChannelWindow(
        queryClient,
        channel.id,
        events,
        previousMessages,
        signal,
      );
    },
    staleTime: 5 * 60 * 1_000,
    gcTime: CONVERSATION_CACHE_GC_MS,
  });
}

export function useChannelSubscription(channel: Channel | null) {
  const queryClient = useQueryClient();
  const channelId = channel?.id ?? null;
  const channelType = channel?.channelType ?? null;
  const [settleTimers] = useState(createSettleTimers);
  // biome-ignore lint/correctness/useExhaustiveDependencies: drop the closed room's follow-up refreshes
  useEffect(() => settleTimers.clear, [channelId, settleTimers]);
  const refreshNewestWindow = useEffectEvent(async () => {
    if (!channelId) return;
    await refreshChannelWindowMessages(queryClient, channelId);
  });

  const appendMessage = useEffectEvent((event: RelayEvent) => {
    if (!channelId) return;
    if (event.kind === KIND_CHANNEL_THREAD_SUMMARY) {
      // Relay-pushed live badge recount — window-store overlay only, never a
      // timeline row (mirrors the page path, where 39005 is metadata).
      const parsed = parseLiveThreadSummary(event);
      if (!parsed) return;
      const windowKey = channelWindowKey(channelId);
      const current =
        queryClient.getQueryData<ChannelWindowStore>(windowKey) ??
        emptyChannelWindowStore();
      const next = mergeLiveThreadSummary(current, parsed.rootId, parsed.live);
      if (next !== current) queryClient.setQueryData(windowKey, next);
      return;
    }
    const isTimelineRow = CHANNEL_TIMELINE_KINDS.has(event.kind);
    const threadReference = isTimelineRow
      ? getThreadReference(event.tags)
      : null;
    if (threadReference?.parentId != null) {
      const rootId = threadReference?.rootId;
      if (rootId) {
        queryClient.setQueryData<RelayEvent[]>(
          threadRepliesKey(channelId, rootId),
          (current = []) => mergeMessages(current, event),
        );
      }
      if (!isBroadcastReply(event.tags)) return;
    }
    if (!isTimelineRow && !CHANNEL_AUX_KINDS.has(event.kind)) return;
    if (!isTimelineRow) {
      queryClient.setQueriesData<RelayEvent[]>(
        { queryKey: ["thread-replies", channelId] },
        (current = []) => mergeMessages(current, event),
      );
    }

    const windowKey = channelWindowKey(channelId);
    const current =
      queryClient.getQueryData<ChannelWindowStore>(windowKey) ??
      emptyChannelWindowStore();
    const next = mergeLiveChannelWindowEvent(current, event, isTimelineRow);
    if (next !== current) {
      queryClient.setQueryData(windowKey, next);
      projectChannelWindowMessages(queryClient, channelId);
    }

    if (event.kind === KIND_SYSTEM_MESSAGE) {
      refreshOnSystemMessage(channelId, event.content, {
        queryClient,
        setTimer: settleTimers.setTimer,
      });
    }
  });

  // Notify the relay client which channel is currently visible so its live
  // subscriptions are replayed first on reconnect, reducing latency on
  // degraded networks.
  useEffect(() => {
    if (!channelId || channelType === "forum") return;
    setVisibleChannel(channelId);
    return () => {
      setVisibleChannel(null);
    };
  }, [channelId, channelType]);

  useEffect(() => {
    if (!channelId || channelType === "forum") {
      return;
    }

    let isDisposed = false;
    let cleanup: (() => Promise<void>) | undefined;
    const disposeReconnectListener = relayClient.subscribeToReconnects(() => {
      void refreshNewestWindow().catch((error) => {
        if (!isDisposed) {
          console.error(
            "Failed to refresh channel window after reconnecting",
            channelId,
            error,
          );
        }
      });
    });

    // The live subscription starts at "now", so it cannot close the gap
    // between the last page snapshot and subscription establishment. Always
    // refresh once subscription setup settles — on success because freshness
    // alone is not proof that no relay events landed in that interval, and on
    // failure because a hydrated channel has no other authoritative fetch:
    // the relay window endpoint may be healthy even when the live socket is
    // not, and the reconnect listener above re-syncs when it recovers.
    const refreshAfterSubscribe = (outcome: string) => {
      if (isDisposed) return;
      void refreshNewestWindow().catch((error) => {
        if (!isDisposed) {
          console.error(
            `Failed to refresh channel window after ${outcome}`,
            channelId,
            error,
          );
        }
      });
    };
    relayClient
      .subscribeToChannelLive(channelId, (event) => {
        if (!isDisposed) {
          appendMessage(event);
        }
      })
      .then(
        (dispose) => {
          if (isDisposed) {
            void dispose();
            return;
          }
          cleanup = dispose;
          refreshAfterSubscribe("subscribing");
        },
        (error) => {
          console.error("Failed to subscribe to channel", channelId, error);
          refreshAfterSubscribe("subscription failure");
        },
      );

    return () => {
      isDisposed = true;
      disposeReconnectListener();
      if (cleanup) {
        void cleanup();
      }
    };
  }, [channelId, channelType]);
}

export function useSendMessageMutation(
  channel: Channel | null,
  identity: Identity | undefined,
) {
  const queryClient = useQueryClient();
  // Retry from a failed row re-runs this mutation even after the screen that
  // sent it unmounted; the variables carry the captured channel.
  const retrySendRef = useRef<
    (variables: SendMessageVariables) => Promise<RelayEvent>
  >(() => Promise.reject(new Error("Send is not ready.")));

  const mutation = useMutation<
    RelayEvent,
    Error,
    SendMessageVariables,
    MessageQueryContext | undefined
  >({
    mutationFn: async ({
      channelId: capturedChannelId,
      targetChannel,
      content,
      mentionPubkeys,
      parentEventId,
      mediaTags,
      forceRest,
      sentFromThreadRootId,
      sentFromThreadRootExcerpt,
      transport = "auto",
      optimisticLocalKey,
    }) => {
      // Prefer a channel captured by the caller at compose time. Otherwise,
      // resolve a captured id from the shared channel cache so navigation
      // cannot redirect the message. Legacy callers without either value use
      // the closed-over `channel`.
      const effectiveChannel = resolveSendChannel(
        targetChannel,
        capturedChannelId,
        queryClient.getQueryData<Channel[]>(channelsQueryKey),
        channel,
      );

      if (effectiveChannel == null) {
        if (capturedChannelId != null) {
          throw new Error("Group is no longer available.");
        }
        throw new Error("This group does not support message sending yet.");
      }

      if (effectiveChannel.channelType === "forum") {
        throw new Error("This group does not support message sending yet.");
      }

      if (!identity) {
        throw new Error("No identity available for sending messages.");
      }

      // `mediaTags` arrives as the merged outgoing tag set (imeta + NIP-30
      // emoji). Split it so each kind goes to its own validated Tauri arg —
      // emoji tags must NOT ride the imeta-only `media` channel (that gate
      // rejects any non-imeta prefix, which silently dropped emoji sends).
      const {
        mediaTags: imetaTags,
        emojiTags,
        mentionTags,
        linkPreviewTags,
        quoteTags,
      } = splitOutgoingTags(mediaTags);
      if (quoteTags.length > 1) {
        throw new Error("A message can quote only one message.");
      }
      const quoteTag = quoteTags[0];
      const recipientPubkeys = messageMentionPubkeys(
        effectiveChannel,
        identity.pubkey,
        mentionPubkeys,
      );
      if (sentFromThreadRootId && parentEventId) {
        throw new Error(
          "A thread message can only be sent as a top-level message.",
        );
      }

      // Sign with the second the sender's row already shows, so the
      // acknowledged message stays exactly where it was drawn.
      const shownSendTime = optimisticLocalKey
        ? findOptimisticMessage(
            queryClient,
            effectiveChannel.id,
            optimisticLocalKey,
          )?.created_at
        : undefined;
      const sentFromThreadTag = sentFromThreadRootId
        ? buildSentFromThreadTag(
            sentFromThreadRootId,
            sentFromThreadRootExcerpt,
          )
        : undefined;

      // Messages carrying media OR custom-emoji tags MUST go through REST so
      // the relay's tag validation runs. The WebSocket path emits no extra
      // tags, so emoji-only messages would otherwise lose their emoji tag.
      // The same holds for a NIP-18 quote tag.
      if (
        forceRest ||
        transport === "http" ||
        parentEventId ||
        imetaTags.length > 0 ||
        emojiTags.length > 0 ||
        linkPreviewTags.length > 0 ||
        quoteTag
      ) {
        const cachedMessages =
          queryClient.getQueryData<RelayEvent[]>(
            channelMessagesKey(effectiveChannel.id),
          ) ?? [];
        const threadCaches = channelThreadCaches(
          queryClient,
          effectiveChannel.id,
        );
        const suppliedRootEventId = parentEventId
          ? resolveCachedReplyRootId(parentEventId, [
              cachedMessages,
              ...threadCaches,
            ])
          : null;
        const result = await sendChannelMessage(
          effectiveChannel.id,
          content,
          parentEventId ?? null,
          imetaTags,
          recipientPubkeys,
          undefined,
          emojiTags,
          mentionTags,
          linkPreviewTags,
          sentFromThreadTag,
          undefined,
          undefined,
          suppliedRootEventId,
          quoteTag,
          undefined,
          shownSendTime,
        );

        // Build tags matching relay-emitted shape: h, author p, mention ps, reply es, imeta, emoji.
        // For replies, buildReplyTags already includes ["p", author] and ["h", channel].
        // For non-replies (media-only), we add them ourselves.
        const replyTags = parentEventId
          ? buildReplyTags(
              effectiveChannel.id,
              identity.pubkey,
              parentEventId,
              result.rootEventId ?? parentEventId,
              recipientPubkeys,
            )
          : [];
        const baseTags = parentEventId
          ? replyTags // buildReplyTags includes h + author p + mention ps
          : [
              ["h", effectiveChannel.id],
              ["p", identity.pubkey],
            ]; // non-reply: add ourselves

        return {
          id: result.eventId,
          pubkey: identity.pubkey,
          created_at: result.createdAt,
          kind: KIND_STREAM_MESSAGE,
          tags: [
            ...baseTags,
            // For non-replies, add mention p-tags here (replies get them via buildReplyTags)
            ...(!parentEventId
              ? normalizeMentionPubkeys(recipientPubkeys, identity.pubkey).map(
                  (pk) => ["p", pk],
                )
              : []),
            ...imetaTags,
            ...emojiTags,
            ...mentionTags,
            ...linkPreviewTags,
            ...(sentFromThreadTag ? [sentFromThreadTag] : []),
            ...(quoteTag ? [quoteTag] : []),
          ],
          content: content.trim(),
          sig: "",
        };
      }

      return relayClient.sendMessage(
        effectiveChannel.id,
        content,
        recipientPubkeys,
        [...mentionTags, ...(sentFromThreadTag ? [sentFromThreadTag] : [])],
        shownSendTime,
      );
    },
    onMutate: async ({
      channelId: capturedChannelId,
      targetChannel,
      content,
      mentionPubkeys,
      parentEventId,
      mediaTags,
      sentFromThreadRootId,
      sentFromThreadRootExcerpt,
      forceRest,
      optimisticLocalKey,
    }) => {
      // Mirror mutationFn's target resolution so the optimistic message lands
      // in the cache for the same channel as the real send. A caller-supplied
      // channel remains valid even when a stale channel-list read omitted it.
      const effectiveChannel = resolveSendChannel(
        targetChannel,
        capturedChannelId,
        queryClient.getQueryData<Channel[]>(channelsQueryKey),
        channel,
      );

      if (
        !effectiveChannel ||
        !identity ||
        effectiveChannel.channelType === "forum"
      ) {
        return undefined;
      }

      const queryKey = channelMessagesKey(effectiveChannel.id);
      const windowKey = channelWindowKey(effectiveChannel.id);
      // The rendered timeline is projected from the channel-window cache. Cancel
      // both reads before snapshotting either cache so an older window response
      // cannot replace the optimistic row between onMutate and onSuccess.
      await Promise.all([
        queryClient.cancelQueries({ queryKey }),
        queryClient.cancelQueries({ queryKey: windowKey }),
      ]);

      const previousMessages =
        queryClient.getQueryData<RelayEvent[]>(queryKey) ?? [];
      const previousWindow =
        queryClient.getQueryData<ChannelWindowStore>(windowKey);
      const optimisticMessage = createOptimisticMessage(
        effectiveChannel.id,
        content.trim(),
        identity,
        [
          ...previousMessages,
          ...channelThreadCaches(queryClient, effectiveChannel.id).flat(),
        ],
        mentionPubkeys ?? [],
        parentEventId ?? null,
        mediaTags ?? [],
        sentFromThreadRootId ?? null,
        sentFromThreadRootExcerpt ?? null,
        optimisticLocalKey,
      );
      // A previewed row is replaced in place: same id and time, so it never
      // moves.
      const previewed = optimisticLocalKey
        ? findOptimisticMessage(
            queryClient,
            effectiveChannel.id,
            optimisticLocalKey,
          )
        : undefined;
      const drawn = previewed
        ? { ...optimisticMessage, created_at: previewed.created_at }
        : optimisticMessage;
      writeOptimisticMessage(queryClient, effectiveChannel.id, drawn);
      if (optimisticLocalKey) {
        // Kept on disk until the relay accepts it, so a failure or a quit
        // mid-send never loses the text.
        saveStoredSend({
          localKey: optimisticLocalKey,
          channelId: effectiveChannel.id,
          rootId: getThreadReference(drawn.tags).rootId,
          event: drawn,
          send: {
            content,
            mentionPubkeys: mentionPubkeys ?? [],
            mediaTags: mediaTags ?? [],
            parentEventId: parentEventId ?? null,
            sentFromThreadRootId: sentFromThreadRootId ?? null,
            sentFromThreadRootExcerpt: sentFromThreadRootExcerpt ?? null,
            forceRest: forceRest ?? false,
          },
        });
      }

      return {
        optimisticId: optimisticMessage.id,
        previousMessages,
        previousWindow,
        channelId: effectiveChannel.id,
        queryKey,
      };
    },
    onError: (error, variables, context) => {
      // A community timeout surfaces here as the relay's `OK false` reason.
      // Record it so the composer can show the timeout chip and block further
      // sends until it expires; other errors fall through to the caller.
      recordTimeoutFromRejection(error?.message);
      const localKey = variables.optimisticLocalKey;
      if (localKey) {
        const channelId = context?.channelId ?? variables.channelId;
        if (!channelId) return;
        if (!context) {
          // Nothing was sent and the row cannot be retried: drop it so the
          // caller restores the draft.
          removeOptimisticMessage(queryClient, channelId, localKey);
          removeStoredSend(localKey);
          return;
        }
        if (hasOwnEcho(queryClient, channelId, localKey)) {
          // Timed out, but the relay stored it: its echo is already shown.
          removeOptimisticMessage(queryClient, channelId, localKey);
          removeStoredSend(localKey);
          return;
        }
        updateOptimisticMessage(queryClient, channelId, localKey, (event) => ({
          ...event,
          failed: true,
        }));
        registerFailedSend(localKey, {
          retry: () => {
            retrySendRef.current(variables).catch(() => {});
          },
          discard: () => {
            removeOptimisticMessage(queryClient, channelId, localKey);
            removeStoredSend(localKey);
          },
        });
        settleStoredSend(localKey);
        return;
      }
      if (!context) {
        return;
      }

      queryClient.setQueryData(context.queryKey, context.previousMessages);
      queryClient.setQueryData(
        channelWindowKey(context.channelId),
        context.previousWindow,
      );
    },
    onSuccess: (message, _variables, context) => {
      // An accepted send proves the write-block is lifted; clear any recorded
      // timeout so the chip and disable state fall away immediately.
      clearTimeoutState();
      if (!context) {
        return;
      }
      removeStoredSend(context.optimisticId);

      const windowKey = channelWindowKey(context.channelId);
      const current =
        queryClient.getQueryData<ChannelWindowStore>(windowKey) ??
        emptyChannelWindowStore();
      const withoutPending: ChannelWindowStore = {
        ...current,
        liveOverlay: current.liveOverlay.filter(
          (event) => event.id !== context.optimisticId,
        ),
      };
      const confirmed = { ...message, localKey: context.optimisticId };
      const next = mergeLiveChannelWindowEvent(withoutPending, confirmed);
      queryClient.setQueryData(windowKey, next);
      projectChannelWindowMessages(queryClient, context.channelId);
      const { rootId } = getThreadReference(confirmed.tags);
      if (rootId) {
        const repliesKey = threadRepliesKey(context.channelId, rootId);
        const replies = queryClient.getQueryData<RelayEvent[]>(repliesKey);
        if (replies?.some((event) => event.id === context.optimisticId)) {
          queryClient.setQueryData(
            repliesKey,
            mergeMessages(replies, confirmed),
          );
        }
      }
    },
  });
  retrySendRef.current = mutation.mutateAsync;
  return mutation;
}

export function useToggleReactionMutation() {
  const queryClient = useQueryClient();
  return useMutation<
    void,
    Error,
    {
      eventId: string;
      emoji: string;
      remove: boolean;
    }
  >({
    mutationFn: async ({ eventId, emoji, remove }) => {
      if (remove) {
        await removeReaction(eventId, emoji);
        return;
      }

      // Custom-emoji reaction: emoji is `:shortcode:`. Resolve its image URL
      // from the cached community palette so the kind:7 carries the NIP-30
      // `["emoji", shortcode, url]` tag. Unicode reactions resolve to no URL.
      const emojiUrl = reactionEmojiUrl(
        emoji,
        queryClient.getQueryData<CustomEmoji[]>(customEmojiQueryKey),
      );
      await addReaction(eventId, emoji, emojiUrl);
    },
  });
}

export function useDeleteMessageMutation(channel: Channel | null) {
  const queryClient = useQueryClient();

  return useMutation<void, Error, { eventId: string }>({
    mutationFn: async ({ eventId }) => {
      if (!channel) {
        throw new Error("No group selected.");
      }
      cancelAutoRoute(eventId);
      await deleteMessage(channel.id, eventId);
    },
    onSuccess: (_data, { eventId }) => {
      if (!channel) return;
      queryClient.setQueryData<RelayEvent[]>(
        channelMessagesKey(channel.id),
        (current = []) => current.filter((message) => message.id !== eventId),
      );
      void queryClient.invalidateQueries({
        queryKey: INBOX_DELETIONS_QUERY_KEY,
      });
    },
    onError: (error) => {
      toast.error(t("messages.delete.failed", { error: error.message }));
    },
  });
}

export function useEditMessageMutation(channel: Channel | null) {
  const queryClient = useQueryClient();

  return useMutation<
    void,
    Error,
    {
      eventId: string;
      content: string;
      mediaTags?: string[][];
      // Pubkeys of mentions *newly added* by this edit, diffed at the composer.
      // Only these receive a `p` tag so a typo-fix edit re-wakes nobody.
      mentionPubkeys?: string[];
    }
  >({
    mutationFn: async ({ eventId, content, mediaTags, mentionPubkeys }) => {
      if (!channel) {
        throw new Error("No group selected.");
      }
      // A Smart routing pick still in flight must not land on the old text.
      cancelAutoRoute(eventId);

      // `mediaTags` arrives as the merged outgoing set (imeta + NIP-30 emoji).
      // Split so each rides its own validated Tauri arg — emoji tags must NOT
      // go through the imeta-only `mediaTags` channel (the Rust `imeta_tags`
      // guard rejects any non-imeta prefix), mirroring the send path.
      const {
        mediaTags: imetaTags,
        emojiTags,
        mentionTags,
      } = splitOutgoingTags(mediaTags);

      await editMessage(
        channel.id,
        eventId,
        content,
        imetaTags,
        emojiTags,
        mentionPubkeys,
        false,
        mentionTags,
      );
    },
    onSuccess: (_data, { eventId, content, mediaTags, mentionPubkeys }) => {
      if (!channel) {
        return;
      }

      // Apply-on-success cache update: reflect the edit's new content and
      // imeta tag set immediately, so the local cache matches what the
      // receiver overlay (formatTimelineMessages) will produce when the
      // edit event arrives back from the relay. (Not a true optimistic
      // update — runs in onSuccess, not onMutate. Worth bearing the cost
      // only because the edit event round-trip can lag perceptibly.)
      const applyEdit = (message: RelayEvent): RelayEvent => {
        if (message.id !== eventId) return message;
        const editTags = [
          ...(mediaTags ?? []),
          ...(mentionPubkeys ?? []).map((pubkey) => ["p", pubkey]),
          ["kuzz:mention-snapshot"],
        ];
        const nextTags =
          mediaTags !== undefined || editTags.length > 0
            ? applyEditTagOverlay(message.tags, editTags)
            : message.tags;
        return { ...message, content, tags: nextTags };
      };

      // The WINDOW STORE is the source of truth: every live merge
      // re-flattens it over `channelMessagesKey`, so patching only the
      // flattened array gets reverted by the next live event (see
      // mapChannelWindowEvents). Update the store first, then keep the
      // flattened cache in step for immediate paint.
      queryClient.setQueryData<ChannelWindowStore>(
        channelWindowKey(channel.id),
        (current) =>
          current ? mapChannelWindowEvents(current, applyEdit) : current,
      );
      queryClient.setQueryData<RelayEvent[]>(
        channelMessagesKey(channel.id),
        (current = []) => current.map(applyEdit),
      );
    },
  });
}
