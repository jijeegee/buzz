/**
 * The root of a thread opened by its root id
 * (docs/message-sync.md#opening-a-thread-from-its-root-id). The panel draws
 * without it: the root only fills the top row, agent pins and the head's
 * actions. Mobile counterpart: `thread_root_provider.dart`.
 */
import * as React from "react";
import { type QueryClient, useQueryClient } from "@tanstack/react-query";

import { storeScope } from "@/features/messages/lib/paintCoverage";
import {
  getRememberedRouteEvent,
  rememberRouteEvents,
} from "@/features/messages/lib/routeEventCache";
import { storeOpensEnabled } from "@/features/messages/lib/storedWindows";
import {
  type ThreadRootState,
  threadRootVerdict,
} from "@/features/messages/lib/threadRootResolve";
import { relayClient } from "@/shared/api/relayClient";
import type { ConnectionState } from "@/shared/api/relayClientShared";
import { invokeTauri } from "@/shared/api/tauri";
import type { RelayEvent } from "@/shared/api/types";

export type { ThreadRootState };

/** The store's copy of a root and whether a stored deletion removes it. */
export type StoredThreadRoot = { event: RelayEvent | null; deleted: boolean };

type ThreadRootDeps = {
  /** The stored root (null when neither it nor a removal is stored). */
  stored: (
    queryClient: QueryClient,
    channelId: string,
    rootId: string,
  ) => Promise<StoredThreadRoot | null>;
  /** The relay's answer to the root and deletion filters; null = failed. */
  relay: (
    queryClient: QueryClient,
    channelId: string,
    rootId: string,
  ) => Promise<RelayEvent[] | null>;
  connectionState: () => ConnectionState;
  /** Fires at once with the current state, then on every change. */
  subscribeToConnectionState: (
    listener: (state: ConnectionState) => void,
  ) => () => void;
};

/** A failed request while connected is asked again after these delays. */
export const THREAD_ROOT_RETRY_DELAYS_MS = [2_000, 5_000, 15_000] as const;

const defaultDeps: ThreadRootDeps = {
  stored: async (queryClient, channelId, rootId) => {
    const scope = storeScope(queryClient);
    if (!storeOpensEnabled() || !scope) return null;
    return invokeTauri<StoredThreadRoot | null>("message_store_thread_root", {
      channelId,
      rootId,
      account: scope.pubkey,
      relayUrl: scope.relayUrl,
    });
  },
  relay: async (queryClient, channelId, rootId) => {
    // Asked as the account and server this cache belongs to; the command
    // refuses once they are no longer the active ones.
    const scope = storeScope(queryClient);
    if (!scope) return null;
    try {
      return await invokeTauri<RelayEvent[]>("get_thread_root", {
        channelId,
        rootId,
        account: scope.pubkey,
        relayUrl: scope.relayUrl,
      });
    } catch (error) {
      console.warn("[ThreadRoot] relay root unavailable", rootId, error);
      return null;
    }
  },
  connectionState: () => relayClient.getConnectionState(),
  subscribeToConnectionState: (listener) =>
    relayClient.subscribeToConnectionState(listener),
};

let deps = defaultDeps;

/** Replace the store, relay and connection seams (tests). Returns restore. */
export function setThreadRootDeps(next: Partial<ThreadRootDeps>) {
  const previous = deps;
  deps = { ...deps, ...next };
  return () => {
    deps = previous;
  };
}

type Resolved = ThreadRootState & { key: string | null };
const EMPTY: Resolved = { key: null, event: null, deleted: false };

/**
 * Resolves `rootId` in `channelId`: the room's loaded events or the
 * remembered copy seed it at once; the store by id and the relay are asked
 * together, and the relay's answer replaces a stored copy. A failed request
 * while connected is retried after 2, 5 and 15 s; after that, and for an
 * empty answer, the relay is asked again whenever the connection becomes
 * connected, until it answers with the root or a deletion. A stored deletion
 * stays even when the relay still returns the root.
 */
export function useThreadRoot({
  channelId,
  enabled,
  loadedEvents,
  rootId,
}: {
  channelId: string | null;
  enabled: boolean;
  loadedEvents: readonly RelayEvent[];
  rootId: string | null;
}): ThreadRootState {
  const queryClient = useQueryClient();
  const key = enabled && channelId && rootId ? `${channelId}/${rootId}` : null;
  const [resolved, setResolved] = React.useState<Resolved>(EMPTY);
  const current = resolved.key === key ? resolved : EMPTY;
  const seed = React.useMemo(() => {
    if (!key || !rootId) return null;
    return (
      loadedEvents.find((event) => event.id === rootId) ??
      getRememberedRouteEvent(queryClient, rootId)
    );
  }, [key, loadedEvents, queryClient, rootId]);
  const event = current.event ?? seed;
  const knownAuthorRef = React.useRef<string | null>(null);
  knownAuthorRef.current = event?.pubkey ?? null;

  React.useEffect(() => {
    if (!key || !channelId || !rootId) return;
    let cancelled = false;
    let answered = false;
    let asking = false;
    let failures = 0;
    let retry: ReturnType<typeof setTimeout> | undefined;
    const update = (apply: (previous: Resolved) => Resolved) =>
      setResolved((previous) =>
        apply(previous.key === key ? previous : { ...EMPTY, key }),
      );

    const readStore = async () => {
      let stored: StoredThreadRoot | null;
      try {
        stored = await deps.stored(queryClient, channelId, rootId);
      } catch (error) {
        console.warn("[ThreadRoot] stored root unavailable", rootId, error);
        return;
      }
      // The relay's answer wins over a late stored one.
      if (!stored || cancelled || answered) return;
      const storedEvent =
        stored.event?.id === rootId &&
        stored.event.tags.some((tag) => tag[0] === "h" && tag[1] === channelId)
          ? stored.event
          : null;
      update((previous) => ({
        key,
        event: previous.event ?? storedEvent,
        deleted: previous.deleted || stored.deleted,
      }));
    };

    const scheduleRetry = () => {
      if (failures >= THREAD_ROOT_RETRY_DELAYS_MS.length) return;
      if (deps.connectionState() !== "connected") return;
      retry = setTimeout(
        () => void askRelay(),
        THREAD_ROOT_RETRY_DELAYS_MS[failures],
      );
      failures += 1;
    };

    const askRelay = async () => {
      if (asking || answered || cancelled) return;
      asking = true;
      clearTimeout(retry);
      let answer: RelayEvent[] | null = null;
      try {
        answer = await deps.relay(queryClient, channelId, rootId);
      } catch {
        answer = null;
      } finally {
        asking = false;
      }
      if (cancelled) return;
      if (answer === null) {
        scheduleRetry();
        return;
      }
      const result = threadRootVerdict(answer, {
        channelId,
        rootId,
        knownRootAuthor: knownAuthorRef.current,
      });
      if (result.verdict === "unresolved") return;
      answered = true;
      if (result.root) rememberRouteEvents(queryClient, [result.root]);
      update((previous) => ({
        key,
        event: result.root ?? previous.event,
        deleted: result.verdict === "deleted" || previous.deleted,
      }));
    };

    let lastState: ConnectionState | null = null;
    const unsubscribe = deps.subscribeToConnectionState((state) => {
      const was = lastState;
      lastState = state;
      if (was !== null && was !== "connected" && state === "connected") {
        failures = 0;
        clearTimeout(retry);
        void askRelay();
      }
    });
    void readStore();
    void askRelay();
    return () => {
      cancelled = true;
      clearTimeout(retry);
      unsubscribe();
    };
  }, [channelId, key, queryClient, rootId]);

  return React.useMemo(
    () => ({ event, deleted: current.deleted }),
    [current.deleted, event],
  );
}
