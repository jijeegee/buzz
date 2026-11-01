import { t } from "@/shared/i18n";
import { useEffect, useRef } from "react";
import {
  type QueryClient,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { relayClient } from "@/shared/api/relayClient";
import { signRelayEvent } from "@/shared/api/tauri";
import type { RelayEvent } from "@/shared/api/types";
import { KIND_THREAD_NAME } from "@/shared/constants/kinds";
import { isValidThreadName } from "./threadName";
import { ThreadNameBatcher } from "./threadNameBatch";
import { mergeThreadName, threadNameQueryKey } from "./threadNameLive";

/**
 * Every thread name read goes through one batcher: the rows on screen share
 * a few reads instead of one each (docs/thread-names.md). Live renames come
 * from the room bundle (`threadNameLive.ts`), not from a REQ per row.
 */
const threadNameBatch = new ThreadNameBatcher({
  fetchEvents: (filter) => relayClient.fetchEvents(filter),
  whenRoomCovered: (roomId) => relayClient.liveRooms.whenRoomCovered(roomId),
});

async function readThreadName(
  queryClient: QueryClient,
  channelId: string,
  threadId: string,
) {
  const fetched = await threadNameBatch.load(channelId, threadId);
  return mergeThreadName(queryClient, channelId, threadId, fetched);
}

/**
 * The read-only thread name query. Shares the `useThreadName` cache key so
 * renames made in an open thread show up immediately.
 */
export function threadNameQueryOptions(
  queryClient: QueryClient,
  channelId: string,
  threadId: string,
) {
  return {
    queryKey: threadNameQueryKey(channelId, threadId),
    staleTime: 5 * 60_000,
    queryFn: () => readThreadName(queryClient, channelId, threadId),
  };
}

/**
 * Read-only thread name for list rows. Kept fresh by the cache-wide live
 * listener, so a row costs no live subscription.
 */
export function useThreadNameLabel(
  channelId: string | null | undefined,
  threadId: string | null | undefined,
) {
  const queryClient = useQueryClient();
  const query = useQuery<RelayEvent | null>({
    ...threadNameQueryOptions(queryClient, channelId ?? "", threadId ?? ""),
    enabled: Boolean(channelId && threadId),
  });
  return query.data?.content ?? "";
}

/** The thread name with its editor actions (open thread panel). */
export function useThreadName(
  channelId: string | null | undefined,
  threadId: string | null | undefined,
) {
  const queryClient = useQueryClient();
  const generation = useRef(0);
  // A save started for one thread must not finish into another.
  // biome-ignore lint/correctness/useExhaustiveDependencies: Bump on every thread change, not only on unmount.
  useEffect(
    () => () => {
      generation.current += 1;
    },
    [channelId, threadId],
  );
  const query = useQuery<RelayEvent | null>({
    queryKey: threadNameQueryKey(channelId ?? "", threadId ?? ""),
    enabled: Boolean(channelId && threadId),
    queryFn: () => {
      if (!channelId || !threadId) throw new Error("No thread selected");
      return readThreadName(queryClient, channelId, threadId);
    },
  });

  return {
    name: query.data?.content ?? "",
    error: query.error,
    retry: () => {
      void query.refetch();
    },
    save: async (input: string) => {
      if (!channelId || !threadId) throw new Error("No thread selected");
      const saveGeneration = generation.current;
      const isCurrent = () => generation.current === saveGeneration;
      const assertCurrent = () => {
        if (!isCurrent())
          throw new Error("Thread changed before the name was saved");
      };
      const name = input.normalize("NFC").trim();
      if (!isValidThreadName(name))
        throw new Error(t("messages.threadName.invalid"));
      const latest = await query.refetch();
      if (latest.error) throw latest.error;
      assertCurrent();
      const event = await signRelayEvent({
        kind: KIND_THREAD_NAME,
        content: name,
        createdAt: Math.max(
          Math.floor(Date.now() / 1000),
          (latest.data?.created_at ?? 0) + 1,
        ),
        tags: [
          ["h", channelId],
          ["e", threadId],
        ],
      });
      assertCurrent();
      await relayClient.publishEvent(
        event,
        t("messages.threadName.saveTimedOut"),
        t("messages.threadName.saveFailed"),
        isCurrent,
      );
      assertCurrent();
      // Token-mode drafts are re-signed by the relay. Only cache the persisted
      // event, never its pre-submit ID (which could win a same-second tie).
      const persisted = await query.refetch();
      assertCurrent();
      if (persisted.error) throw persisted.error;
    },
  };
}
