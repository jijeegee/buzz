import * as React from "react";

import { sendAgentObserverControl } from "@/shared/api/observerRelay";
import { relayClient } from "@/shared/api/relayClient";
import { useDocumentVisible } from "@/shared/lib/useDocumentVisible";
import {
  createObserverWatchingRegistry,
  type ObserverWatchingRegistry,
} from "./observerWatchingRegistry";

let registry: ObserverWatchingRegistry | null = null;

/** The app-wide registry: one `watching` sender per agent. */
export function getObserverWatchingRegistry(): ObserverWatchingRegistry {
  registry ??= createObserverWatchingRegistry({
    send: (agentPubkey, channelId) =>
      sendAgentObserverControl(agentPubkey, { type: "watching", channelId }),
    isConnected: () => relayClient.getConnectionState() === "connected",
    subscribeConnection: (listener) =>
      relayClient.subscribeToConnectionState((state) =>
        listener(state === "connected"),
      ),
    onSendError: (error) => {
      console.debug("Observer watching signal not sent:", error);
    },
  });
  return registry;
}

/** Test-only: drop the registry so the next caller builds a fresh one. */
export function _resetObserverWatchingRegistryForTests() {
  registry = null;
}

/**
 * Tell `agentPubkey`'s executor that its live activity is being watched while
 * `live` is true and the window is visible. A view of archived history only
 * passes `live = false`.
 */
export function useObserverWatching(
  agentPubkey: string | null | undefined,
  channelId: string | null,
  live: boolean,
) {
  const visible = useDocumentVisible();
  React.useEffect(() => {
    if (!live || !visible || !agentPubkey) return;
    return getObserverWatchingRegistry().acquire(agentPubkey, channelId);
  }, [agentPubkey, channelId, live, visible]);
}
