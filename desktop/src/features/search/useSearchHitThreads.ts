import { useQuery, useQueryClient } from "@tanstack/react-query";
import * as React from "react";

import {
  type ChatListRow,
  threadRootTitle,
} from "@/features/chats/chatListState";
import { getThreadReference } from "@/features/messages/lib/threading";
import { threadNameQueryOptions } from "@/features/messages/lib/useThreadName";
import { getEventsByIds } from "@/shared/api/tauri";
import type { Channel, SearchHit } from "@/shared/api/types";
import { CHANNEL_TIMELINE_CONTENT_KINDS } from "@/shared/constants/kinds";

/** The thread a message search hit sits in. */
export type SearchHitThread = { name: string; rootId: string };

const TIMELINE_KINDS: readonly number[] = CHANNEL_TIMELINE_CONTENT_KINDS;

/** A slow lookup must not hold message results back for long. */
const THREAD_LOOKUP_TIMEOUT_MS = 2_000;

type HitRoot = {
  /** The thread name event's name, when the Chats list had none. */
  name: string | null;
  rootContent: string | undefined;
  rootId: string;
};

function withTimeout<T>(promise: Promise<T>, ms: number): Promise<T> {
  let timer: number | undefined;
  return Promise.race([
    promise,
    new Promise<never>((_, reject) => {
      timer = window.setTimeout(
        () => reject(new Error("Thread lookup timed out")),
        ms,
      );
    }),
  ]).finally(() => window.clearTimeout(timer));
}

/**
 * Search hits carry no thread tags, so look up which hits are thread replies
 * (one batch fetch for the hits, one for their roots) and each thread's name:
 * the Chats list name, else the thread name event, else the root's first
 * line, as the Chats thread row shows it. `isLoading` stays true until the
 * lookup settles (bounded), so rows draw once with their final label; a
 * failed or slow lookup shows plain message rows.
 */
export function useSearchHitThreads({
  channelLookup,
  enabled,
  hits,
  threadRows,
}: {
  channelLookup: ReadonlyMap<string, Channel>;
  enabled: boolean;
  hits: readonly SearchHit[];
  threadRows?: readonly ChatListRow[];
}) {
  const queryClient = useQueryClient();
  const listedNames = React.useMemo(() => {
    const names = new Map<string, string>();
    for (const row of threadRows ?? []) {
      const name = row.thread_name?.trim();
      if (row.type === "thread" && row.root_id && name) {
        names.set(row.root_id, name);
      }
    }
    return names;
  }, [threadRows]);
  const listedNamesRef = React.useRef(listedNames);
  listedNamesRef.current = listedNames;

  const candidates = React.useMemo(
    () =>
      hits
        .filter((hit) => {
          const channel = hit.channelId
            ? channelLookup.get(hit.channelId)
            : undefined;
          return (
            hit.channelId &&
            channel?.channelType !== "forum" &&
            TIMELINE_KINDS.includes(hit.kind)
          );
        })
        .map((hit) => ({ channelId: hit.channelId ?? "", id: hit.eventId }))
        .sort((left, right) => left.id.localeCompare(right.id)),
    [channelLookup, hits],
  );
  const query = useQuery({
    enabled: enabled && candidates.length > 0,
    queryKey: ["search-hit-threads", candidates.map(({ id }) => id)],
    retry: false,
    staleTime: 5 * 60_000,
    queryFn: () =>
      withTimeout(
        (async (): Promise<Record<string, HitRoot>> => {
          const channelOf = new Map(
            candidates.map(({ channelId, id }) => [id, channelId]),
          );
          const events = await getEventsByIds(candidates.map(({ id }) => id));
          const rootByEventId = new Map<string, string>();
          for (const event of events) {
            const { rootId } = getThreadReference(event.tags);
            if (rootId && rootId !== event.id) {
              rootByEventId.set(event.id, rootId);
            }
          }
          const channelOfRoot = new Map(
            [...rootByEventId].map(([eventId, rootId]) => [
              rootId,
              channelOf.get(eventId) ?? "",
            ]),
          );
          const unnamed = [...channelOfRoot].filter(
            ([rootId]) => !listedNamesRef.current.has(rootId),
          );
          const [roots, names] = await Promise.all([
            getEventsByIds([...channelOfRoot.keys()]),
            // Shares the Chats row thread-name cache.
            Promise.all(
              unnamed.map(([rootId, channelId]) =>
                queryClient
                  .fetchQuery(threadNameQueryOptions(channelId, rootId))
                  .then(
                    (event) => [rootId, event?.content.trim() || null] as const,
                  )
                  .catch(() => [rootId, null] as const),
              ),
            ),
          ]);
          const rootContent = new Map(
            roots.map((root) => [root.id, root.content]),
          );
          const nameOf = new Map(names);
          return Object.fromEntries(
            [...rootByEventId].map(([eventId, rootId]) => [
              eventId,
              {
                name: nameOf.get(rootId) ?? null,
                rootContent: rootContent.get(rootId),
                rootId,
              },
            ]),
          );
        })(),
        THREAD_LOOKUP_TIMEOUT_MS,
      ),
  });
  const roots = query.data;

  const threadByEventId = React.useMemo(() => {
    const threads = new Map<string, SearchHitThread>();
    for (const [eventId, { name, rootContent, rootId }] of Object.entries(
      roots ?? {},
    )) {
      threads.set(eventId, {
        name: listedNames.get(rootId) ?? name ?? threadRootTitle(rootContent),
        rootId,
      });
    }
    return threads;
  }, [listedNames, roots]);

  return { isLoading: query.isLoading, threadByEventId };
}
