import { useQuery } from "@tanstack/react-query";

import {
  type ChatStateBinding,
  isChatStateUnavailable,
  searchThreadNames,
} from "@/shared/api/chatState";
import { searchMessages } from "@/shared/api/tauri";

export const MIN_SEARCH_QUERY_LENGTH = 2;
export const MIN_SCOPED_SEARCH_QUERY_LENGTH = 1;

export function getMinimumSearchQueryLength(channelId?: string | null) {
  return channelId ? MIN_SCOPED_SEARCH_QUERY_LENGTH : MIN_SEARCH_QUERY_LENGTH;
}

export function useSearchMessagesQuery(
  query: string,
  options?: {
    channelId?: string;
    authors?: string[];
    since?: number | null;
    until?: number | null;
    enabled?: boolean;
    limit?: number;
    unresolvedOperator?: boolean;
    minimumQueryLength?: number;
  },
) {
  const trimmedQuery = query.trim();
  const enabled = options?.enabled ?? true;
  const limit = options?.limit ?? 12;
  const channelId = options?.channelId;
  const authors = options?.authors;
  const since = options?.since ?? null;
  const until = options?.until ?? null;
  const unresolvedOperator = options?.unresolvedOperator ?? false;
  const minimumQueryLength =
    options?.minimumQueryLength ?? getMinimumSearchQueryLength(channelId);

  return useQuery({
    queryKey: [
      "search-messages",
      trimmedQuery,
      limit,
      channelId ?? null,
      authors ?? null,
      since,
      until,
      unresolvedOperator,
    ],
    queryFn: () =>
      searchMessages({
        q: trimmedQuery,
        limit,
        channelId,
        authors,
        since: since ?? undefined,
        until: until ?? undefined,
      }),
    // Call sites own the "when to search" floor (FTS length / unresolved
    // operators). Keep a single threshold here so it cannot drift.
    enabled: enabled && trimmedQuery.length >= minimumQueryLength,
    staleTime: 30_000,
    gcTime: 5 * 60 * 1_000,
  });
}

/**
 * The relay's thread-name search for `query` (already debounced and
 * trimmed by the caller). Off without a chat-state binding, for scoped
 * search, and below the global minimum length. A relay that keeps no chat
 * state answers `chat_state_unavailable`; that is not retried.
 */
export function useThreadNameSearchQuery(
  query: string,
  binding: ChatStateBinding | null | undefined,
  enabled: boolean,
) {
  const trimmedQuery = query.trim();
  return useQuery({
    queryKey: [
      "search-thread-names",
      binding?.relayUrl ?? "",
      binding?.pubkey.toLowerCase() ?? "",
      trimmedQuery,
    ],
    queryFn: () => {
      if (!binding) throw new Error("no chat-state binding");
      return searchThreadNames(binding, trimmedQuery);
    },
    enabled:
      enabled && !!binding && trimmedQuery.length >= MIN_SEARCH_QUERY_LENGTH,
    retry: (failureCount, error) =>
      !isChatStateUnavailable(error) && failureCount < 1,
    staleTime: 30_000,
    gcTime: 5 * 60 * 1_000,
  });
}
