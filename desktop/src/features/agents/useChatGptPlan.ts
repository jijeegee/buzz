import * as React from "react";
import { QueryClientContext } from "@tanstack/react-query";

import {
  getChatGptPlanStatus,
  signInChatGptPlan,
  signOutChatGptPlan,
  type ChatGptPlanSignIn,
  type ChatGptPlanStatus,
} from "@/shared/api/tauriChatGptPlan";

import { useReturnedToApp } from "@/shared/hooks/useReturnedToApp";

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

function reloadOnFocus() {
  void load();
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  // Agents can end a dead session on their own; re-read on every mount and
  // whenever the window regains focus so Settings never shows a stale state.
  void load();
  if (listeners.size === 1) window.addEventListener("focus", reloadOnFocus);
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0) {
      window.removeEventListener("focus", reloadOnFocus);
    }
  };
}

export function useChatGptPlanStatus(): Snapshot {
  return React.useSyncExternalStore(
    subscribe,
    () => snapshot,
    () => snapshot,
  );
}

type ActionState<T> = {
  isPending: boolean;
  error: unknown;
  data: T | undefined;
};

/** Wraps sign-in/out with pending/error state and refreshes dependents. */
function useAuthAction<T>(run: () => Promise<T>) {
  const queryClient = React.useContext(QueryClientContext);
  const [state, setState] = React.useState<ActionState<T>>({
    isPending: false,
    error: null,
    data: undefined,
  });
  // Only the latest attempt owns the state; a released one settles silently.
  const latest = React.useRef(0);

  const mutateAsync = React.useCallback(async () => {
    const attempt = ++latest.current;
    const settle = (next: ActionState<T>) => {
      if (attempt === latest.current) setState(next);
    };
    settle({ isPending: true, error: null, data: undefined });
    try {
      const data = await run();
      settle({ isPending: false, error: null, data });
      return data;
    } catch (error) {
      settle({ isPending: false, error, data: undefined });
      throw error;
    } finally {
      await load();
      // Readiness ("sign in with ChatGPT") is part of each managed agent.
      void queryClient?.invalidateQueries({ queryKey: managedAgentsQueryKey });
    }
  }, [queryClient, run]);

  /** Stop showing the running attempt; it keeps going in the background. */
  const release = React.useCallback(() => {
    latest.current += 1;
    setState({ isPending: false, error: null, data: undefined });
  }, []);

  return {
    ...state,
    isError: state.error !== null,
    isSuccess:
      !state.isPending && state.error === null && state.data !== undefined,
    mutateAsync,
    mutate: () => void mutateAsync().catch(() => {}),
    release,
  };
}

/**
 * Returning to Kuzz frees the button while the sign-in keeps listening, so
 * finishing in the browser later still connects. Signing in again replaces it.
 */
export function useChatGptPlanSignIn() {
  const action = useAuthAction<ChatGptPlanSignIn>(signInChatGptPlan);
  useReturnedToApp(action.isPending, action.release);
  return action;
}

export function useChatGptPlanSignOut() {
  return useAuthAction<boolean>(signOutChatGptPlan);
}
