import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useRef } from "react";

import { getForumPosts, getForumThread } from "@/shared/api/forum";
import { useFocusedRefetchInterval } from "@/shared/lib/useDocumentVisible";
import {
  lookupStoredSendOnRelay,
  sendTimeOfDrawnRow,
  type StoredSendRelayCheck,
} from "@/features/messages/hooks";
import { registerFailedSend } from "@/features/messages/lib/failedSends";
import {
  removeStoredSend,
  saveStoredSend,
  settleStoredSend,
} from "@/features/messages/lib/failedSendStore";
import { useRelaySelfQuery } from "@/features/moderation/hooks";
import { deleteMessage, sendChannelMessage } from "@/shared/api/tauri";
import type {
  Channel,
  ForumPostsResponse,
  ForumThreadResponse,
  SendChannelMessageResult,
} from "@/shared/api/types";
import { t } from "@/shared/i18n";

import {
  buildForumLocalEvent,
  type ForumLocalSend,
  type ForumSendVariables,
  findForumLocalSend,
  forumLocalSendsKey,
  hasForumCopy,
  readForumLocalSends,
  removeForumLocalSend,
  removeForumLocalSendOfEvent,
  syncForumLocalSends,
  updateForumLocalSend,
  writeForumLocalSend,
} from "./lib/localSends";

/** Keeps focused polling for forum posts at the established 15-second cadence. */
export const FORUM_POSTS_REFETCH_INTERVAL_MS = 15_000;
/** Keeps focused polling for forum threads at the established 10-second cadence. */
export const FORUM_THREAD_REFETCH_INTERVAL_MS = 10_000;
/** Suppresses the focus refetch until forum data is genuinely stale.
 * Both families poll while focused and have push-invalidation from mutations. */
export const FORUM_FOCUS_STALE_TIME_MS = 5 * 60_000;

/** Focus-refetch policy shared by forum-posts and forum-thread queries; consumed by focusRefetchPolicy.test.mjs. */
export const forumFocusRefetchPolicy = {
  staleTime: FORUM_FOCUS_STALE_TIME_MS,
  refetchOnWindowFocus: false,
} as const;

export function forumPostsQueryKey(channelId: string) {
  return ["forum-posts", channelId] as const;
}

export function forumThreadQueryKey(channelId: string, eventId: string) {
  return ["forum-thread", channelId, eventId] as const;
}

export function useForumPostsQuery(channel: Channel | null) {
  const refetchInterval = useFocusedRefetchInterval(
    FORUM_POSTS_REFETCH_INTERVAL_MS,
  );

  const channelId = channel?.id ?? "";
  const enabled = channel !== null && channel.channelType === "forum";
  const relaySelfPubkey = useRelaySelfQuery(enabled).data;

  return useQuery<ForumPostsResponse>({
    enabled,
    queryKey: [...forumPostsQueryKey(channelId), relaySelfPubkey ?? null],
    queryFn: () => getForumPosts(channelId, 50, undefined, relaySelfPubkey),
    refetchInterval,
    ...forumFocusRefetchPolicy,
  });
}

export function useForumThreadQuery(
  channelId: string | null,
  eventId: string | null,
) {
  const refetchInterval = useFocusedRefetchInterval(
    FORUM_THREAD_REFETCH_INTERVAL_MS,
  );

  const enabled = channelId !== null && eventId !== null;
  const relaySelfPubkey = useRelaySelfQuery(enabled).data;

  return useQuery<ForumThreadResponse>({
    enabled,
    queryKey: [
      ...forumThreadQueryKey(channelId ?? "", eventId ?? ""),
      relaySelfPubkey ?? null,
    ],
    queryFn: () =>
      getForumThread(
        channelId ?? "",
        eventId ?? "",
        undefined,
        undefined,
        relaySelfPubkey,
      ),
    refetchInterval,
    ...forumFocusRefetchPolicy,
  });
}

export function useDeleteForumPostMutation(channel: Channel | null) {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async ({ eventId }: { eventId: string }) => {
      if (!channel) {
        throw new Error(t("forum.noGroupSelected"));
      }
      await deleteMessage(channel.id, eventId);
    },
    onSuccess: (_data, { eventId }) => {
      if (channel) {
        // A just-sent post no fetch has returned yet would otherwise stay.
        removeForumLocalSendOfEvent(queryClient, channel.id, eventId);
        void queryClient.invalidateQueries({
          queryKey: forumPostsQueryKey(channel.id),
        });
      }
    },
  });
}

export function useDeleteForumReplyMutation(
  channel: Channel | null,
  rootEventId: string | null,
) {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async ({ eventId }: { eventId: string }) => {
      if (!channel) {
        throw new Error(t("forum.noGroupSelected"));
      }
      await deleteMessage(channel.id, eventId);
    },
    onSuccess: (_data, { eventId }) => {
      if (channel) {
        removeForumLocalSendOfEvent(queryClient, channel.id, eventId);
        if (rootEventId) {
          void queryClient.invalidateQueries({
            queryKey: forumThreadQueryKey(channel.id, rootEventId),
          });
        }
        void queryClient.invalidateQueries({
          queryKey: forumPostsQueryKey(channel.id),
        });
      }
    },
  });
}

/**
 * Send own forum posts and replies the way chat sends messages: `send` draws
 * the row at once, at the second it is signed with, and never waits for the
 * relay. A failed send stays as a row with Retry / Delete, and its text is
 * kept on disk until the relay accepts it. `send` throws only when it could
 * not draw the row, so the composer keeps its text.
 */
export function useForumSend(pubkey: string | undefined) {
  const queryClient = useQueryClient();
  // Retry from a failed row re-runs this mutation even after the view that
  // sent it unmounted; the variables carry the channel.
  const retryRef = useRef<(variables: ForumSendVariables) => Promise<unknown>>(
    () => Promise.reject(new Error("Send is not ready.")),
  );

  const mutation = useMutation<
    SendChannelMessageResult,
    Error,
    ForumSendVariables,
    { channelId: string } | undefined
  >({
    mutationFn: (variables) =>
      sendChannelMessage(
        variables.channelId,
        variables.content,
        variables.parentEventId,
        variables.mediaTags,
        variables.mentionPubkeys,
        variables.kind,
        undefined,
        undefined,
        undefined,
        undefined,
        undefined,
        undefined,
        undefined,
        undefined,
        undefined,
        // Sign with the second the row already shows, so it never moves.
        findForumLocalSend(queryClient, variables.channelId, variables.localKey)
          ?.event.created_at,
      ),
    onMutate: (variables) => {
      const { channelId, localKey, parentEventId } = variables;
      const now = Math.floor(Date.now() / 1000);
      const shown = findForumLocalSend(queryClient, channelId, localKey)?.event;
      // A Retry long after the row was drawn moves it to now: the backend
      // signs it then, and its copy must match the row and the disk copy.
      const drawn = shown
        ? { ...shown, created_at: sendTimeOfDrawnRow(shown.created_at, now) }
        : pubkey
          ? buildForumLocalEvent(variables, pubkey, now)
          : null;
      if (!drawn) return undefined;
      writeForumLocalSend(queryClient, {
        localKey,
        channelId,
        parentEventId,
        event: drawn,
        eventId: null,
        failed: false,
      });
      // Kept on disk until the relay accepts it, so a failure or a quit
      // mid-send never loses the text.
      saveStoredSend({
        localKey,
        channelId,
        rootId: parentEventId,
        event: drawn,
        send: {
          content: variables.content,
          mentionPubkeys: variables.mentionPubkeys,
          mediaTags: variables.mediaTags,
          parentEventId,
          sentFromThreadRootId: null,
          sentFromThreadRootExcerpt: null,
          forceRest: false,
        },
      });
      return { channelId };
    },
    onSuccess: (result, { channelId, localKey, parentEventId }) => {
      removeStoredSend(localKey);
      // The row stays, without a send state, until a fetch returns the event.
      updateForumLocalSend(queryClient, channelId, localKey, (send) => ({
        ...send,
        eventId: result.eventId,
        event: { ...send.event, created_at: result.createdAt },
        failed: false,
      }));
      if (parentEventId) {
        void queryClient.invalidateQueries({
          queryKey: forumThreadQueryKey(channelId, parentEventId),
        });
      }
      void queryClient.invalidateQueries({
        queryKey: forumPostsQueryKey(channelId),
      });
    },
    onError: (_error, variables, context) => {
      const { channelId, localKey } = variables;
      if (!context || hasForumCopy(queryClient, channelId, localKey)) {
        // Nothing was drawn, or it timed out after the relay stored it and
        // its copy is already shown.
        removeForumLocalSend(queryClient, channelId, localKey);
        removeStoredSend(localKey);
        return;
      }
      updateForumLocalSend(queryClient, channelId, localKey, (send) => ({
        ...send,
        failed: true,
      }));
      registerFailedSend(localKey, {
        retry: () => {
          retryRef.current(variables).catch(() => {});
        },
        discard: () => {
          removeForumLocalSend(queryClient, channelId, localKey);
          removeStoredSend(localKey);
        },
      });
      settleStoredSend(localKey);
    },
  });
  retryRef.current = mutation.mutateAsync;

  const { mutate } = mutation;
  const send = useCallback(
    (input: Omit<ForumSendVariables, "localKey">) => {
      if (!pubkey) throw new Error(t("profile.actions.noIdentity"));
      const variables: ForumSendVariables = {
        ...input,
        localKey: `forum-local-${crypto.randomUUID()}`,
      };
      writeForumLocalSend(queryClient, {
        localKey: variables.localKey,
        channelId: variables.channelId,
        parentEventId: variables.parentEventId,
        event: buildForumLocalEvent(
          variables,
          pubkey,
          Math.floor(Date.now() / 1000),
        ),
        eventId: null,
        failed: false,
      });
      mutate(variables);
    },
    [mutate, pubkey, queryClient],
  );

  return { send, retry: mutation.mutateAsync };
}

/**
 * Own forum rows of a channel, kept in step with what the forum loads: rows
 * a fetch returned go, and sends an earlier app session never got
 * acknowledged come back as failed rows.
 */
export function useForumLocalSends(
  channelId: string,
  retry: (variables: ForumSendVariables) => Promise<unknown>,
): ForumLocalSend[] {
  const queryClient = useQueryClient();
  const retryRef = useRef(retry);
  retryRef.current = retry;

  useEffect(() => {
    let disposed = false;
    const relayCheck: StoredSendRelayCheck = {
      state: new Map(),
      lookup: lookupStoredSendOnRelay,
      onSettled: () => {
        if (!disposed) sync();
      },
      isDisposed: () => disposed,
    };
    let syncing = false;
    const sync = () => {
      if (syncing) return;
      syncing = true;
      try {
        syncForumLocalSends(
          queryClient,
          channelId,
          (variables) => retryRef.current(variables),
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
        (scope === "forum-posts" || scope === "forum-thread") &&
        // Data changes only; observer bookkeeping fires on every render.
        (event.type === "added" ||
          (event.type === "updated" &&
            (event.action.type === "success" ||
              event.action.type === "setState")))
      ) {
        sync();
      }
    });
    return () => {
      disposed = true;
      unsubscribe();
    };
  }, [channelId, queryClient]);

  return useQuery({
    queryKey: forumLocalSendsKey(channelId),
    queryFn: () => readForumLocalSends(queryClient, channelId),
    initialData: [],
    // Written only through the cache; never refetched, and kept while sends
    // can still settle into it.
    staleTime: Number.POSITIVE_INFINITY,
    gcTime: Number.POSITIVE_INFINITY,
  }).data;
}
