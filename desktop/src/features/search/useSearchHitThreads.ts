import { useQueries, useQuery } from "@tanstack/react-query";
import * as React from "react";

import {
  type ChatListRow,
  threadRootTitle,
} from "@/features/chats/chatListState";
import { getThreadReference } from "@/features/messages/lib/threading";
import { threadNameQueryOptions } from "@/features/messages/lib/useThreadName";
import { relayClient } from "@/shared/api/relayClient";
import type { Channel, SearchHit } from "@/shared/api/types";
import { CHANNEL_TIMELINE_CONTENT_KINDS } from "@/shared/constants/kinds";

/** The thread a message search hit sits in. */
export type SearchHitThread = { name: string; rootId: string };

const TIMELINE_KINDS: readonly number[] = CHANNEL_TIMELINE_CONTENT_KINDS;

type HitRoot = { rootId: string; rootContent: string | undefined };

/**
 * Search hits carry no thread tags, so look up which hits are thread replies
 * (one relay fetch for all hits, plus one for their roots) and each thread's
 * name: the Chats list name when loaded, else the thread name event, else the
 * root's first line, as the Chats thread row shows it. `isLoading` stays true
 * until every name is known, so rows draw once with their final label.
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
  const candidateIds = React.useMemo(
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
        .map((hit) => hit.eventId)
        .sort(),
    [channelLookup, hits],
  );
  const rootsQuery = useQuery({
    enabled: enabled && candidateIds.length > 0,
    queryKey: ["search-hit-threads", candidateIds],
    staleTime: 5 * 60_000,
    queryFn: async (): Promise<Record<string, HitRoot>> => {
      const events = await relayClient.fetchEvents({
        ids: candidateIds,
        kinds: [...TIMELINE_KINDS],
        limit: candidateIds.length,
      });
      const rootByEventId = new Map<string, string>();
      for (const event of events) {
        const { rootId } = getThreadReference(event.tags);
        if (rootId && rootId !== event.id) rootByEventId.set(event.id, rootId);
      }
      const rootIds = [...new Set(rootByEventId.values())];
      const roots =
        rootIds.length > 0
          ? await relayClient.fetchEvents({
              ids: rootIds,
              kinds: [...TIMELINE_KINDS],
              limit: rootIds.length,
            })
          : [];
      const rootContent = new Map(roots.map((root) => [root.id, root.content]));
      return Object.fromEntries(
        [...rootByEventId].map(([eventId, rootId]) => [
          eventId,
          { rootId, rootContent: rootContent.get(rootId) },
        ]),
      );
    },
  });
  const roots = rootsQuery.data;

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

  const unnamedThreads = React.useMemo(() => {
    const threads = new Map<string, string>();
    for (const hit of hits) {
      const rootId = roots?.[hit.eventId]?.rootId;
      if (rootId && hit.channelId && !listedNames.has(rootId)) {
        threads.set(rootId, hit.channelId);
      }
    }
    return [...threads].map(([rootId, channelId]) => ({ channelId, rootId }));
  }, [hits, listedNames, roots]);
  const nameQueries = useQueries({
    queries: unnamedThreads.map(({ channelId, rootId }) => ({
      ...threadNameQueryOptions(channelId, rootId),
      enabled,
    })),
  });

  const threadByEventId = React.useMemo(() => {
    const fetchedNames = new Map<string, string>();
    unnamedThreads.forEach(({ rootId }, index) => {
      const name = nameQueries[index]?.data?.content?.trim();
      if (name) fetchedNames.set(rootId, name);
    });
    const threads = new Map<string, SearchHitThread>();
    for (const [eventId, { rootId, rootContent }] of Object.entries(
      roots ?? {},
    )) {
      threads.set(eventId, {
        name:
          listedNames.get(rootId) ??
          fetchedNames.get(rootId) ??
          threadRootTitle(rootContent),
        rootId,
      });
    }
    return threads;
  }, [listedNames, nameQueries, roots, unnamedThreads]);

  return {
    isLoading:
      rootsQuery.isLoading || nameQueries.some((query) => query.isLoading),
    threadByEventId,
  };
}
