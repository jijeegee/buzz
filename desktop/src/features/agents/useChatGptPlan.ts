import * as React from "react";
import { QueryClientContext } from "@tanstack/react-query";

import {
  getChatGptPlanStatus,
  signInChatGptPlan,
  signOutChatGptPlan,
  type ChatGptPlanSignIn,
  type ChatGptPlanStatus,
} from "@/shared/api/tauriChatGptPlan";

import { managedAgentsQueryKey } from "./hooks";

/**
 * Kuzz's ChatGPT plan sign-in state, shared by Settings and every agent
 * dialog. A module store (not React Query) so field components stay usable
 * without a QueryClientProvider, as their tests render them.
 */
type Snapshot = { data: ChatGptPlanStatus | undefined; isPending: boolean };

let snapshot: Snapshot = { data: undefined, isPending: true };
let loading: Promise<void> | null = null;
const listeners = new Set<() => void>();

function publish(next: Snapshot) {
  snapshot = next;
  for (const listener of listeners) listener();
}

function load(): Promise<void> {
  loading ??= getChatGptPlanStatus()
    .then((data) => publish({ data, isPending: false }))
    .catch(() => publish({ data: snapshot.data, isPending: false }))
    .finally(() => {
      loading = null;
    });
  return loading;
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  if (snapshot.isPending) void load();
  return () => listeners.delete(listener);
}

export function useChatGptPlanStatus(): Snapshot {
  return React.useSyncExternalStore(
    subscribe,
    () => snapshot,
    () => snapshot,
  );
}

/** Wraps sign-in/out with pending/error state and refreshes dependents. */
function useAuthAction<T>(run: () => Promise<T>) {
  const queryClient = React.useContext(QueryClientContext);
  const [state, setState] = React.useState<{
    isPending: boolean;
    error: unknown;
    data: T | undefined;
  }>({ isPending: false, error: null, data: undefined });

  const mutateAsync = React.useCallback(async () => {
    setState({ isPending: true, error: null, data: undefined });
    try {
      const data = await run();
      setState({ isPending: false, error: null, data });
      return data;
    } catch (error) {
      setState({ isPending: false, error, data: undefined });
      throw error;
    } finally {
      await load();
      // Readiness ("sign in with ChatGPT") is part of each managed agent.
      void queryClient?.invalidateQueries({ queryKey: managedAgentsQueryKey });
    }
  }, [queryClient, run]);

  return {
    ...state,
    isError: state.error !== null,
    isSuccess:
      !state.isPending && state.error === null && state.data !== undefined,
    mutateAsync,
    mutate: () => void mutateAsync().catch(() => {}),
  };
}

export function useChatGptPlanSignIn() {
  return useAuthAction<ChatGptPlanSignIn>(signInChatGptPlan);
}

export function useChatGptPlanSignOut() {
  return useAuthAction<boolean>(signOutChatGptPlan);
}
