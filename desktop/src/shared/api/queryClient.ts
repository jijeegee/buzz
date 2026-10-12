import { focusManager, QueryClient } from "@tanstack/react-query";

import { isQueryDeadlineError } from "@/shared/lib/relayError";
import {
  isAppFocused,
  subscribeAppFocus,
} from "@/shared/lib/useDocumentVisible";

let focusManagerConfigured = false;

function configureQueryFocusManager() {
  if (focusManagerConfigured) return;
  focusManagerConfigured = true;

  // Treat app blur as unfocused so query retries pause and stale queries with
  // refetchOnWindowFocus refresh on return. Mutations are unaffected by this
  // focus gate; presence heartbeats also explicitly use retry: 0.
  focusManager.setEventListener((setFocused) => {
    setFocused(isAppFocused());
    return subscribeAppFocus(setFocused);
  });
}

/**
 * Conversation caches (room pages, thread replies) stay in memory this long
 * after they are last on screen, so rooms and threads opened earlier in the
 * day reopen at once instead of reloading.
 */
export const CONVERSATION_CACHE_GC_MS = 12 * 60 * 60 * 1_000;

const CONVERSATION_CACHE_KEYS = [
  ["channel-messages"],
  ["channel-window"],
  ["thread-replies"],
  ["thread-window-state"],
] as const;

export function createBuzzQueryClient() {
  configureQueryFocusManager();
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: {
        // One retry, except after the relay's `query timed out` answer: that
        // would re-run the same slow server-side query.
        retry: (failureCount, error) =>
          failureCount < 1 && !isQueryDeadlineError(error),
        refetchOnWindowFocus: false,
        networkMode: "always",
        gcTime: 5 * 60 * 1_000,
      },
      mutations: {
        networkMode: "always",
      },
    },
  });
  // Defaults by key also cover caches written with setQueryData (background
  // preload, live events) before any screen observes them.
  for (const queryKey of CONVERSATION_CACHE_KEYS) {
    queryClient.setQueryDefaults(queryKey, {
      gcTime: CONVERSATION_CACHE_GC_MS,
    });
  }
  return queryClient;
}
