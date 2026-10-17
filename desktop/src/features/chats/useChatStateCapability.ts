import * as React from "react";

import {
  chatStateCapabilityKey,
  getChatStateCapability,
  probeChatStateCapability,
  subscribeChatStateCapability,
  type ChatStateCapability,
} from "@/features/chats/chatStateCapability";
import { getChatListPage } from "@/shared/api/chatState";
import { relayClient } from "@/shared/api/relayClient";

/** Window focus re-probes at most this often. */
export const CHAT_STATE_FOCUS_PROBE_MS = 5 * 60_000;

/**
 * The relay's chat-state capability for this relay + pubkey, probed with a
 * one-row `chat_list` while connected and not known to be on: at start, on
 * every reconnect, and on window focus (throttled). `reprobe` is for the
 * chat-state ping. While on, the server list query keeps checking: its
 * "unavailable" error turns the capability off.
 */
export function useChatStateCapability({
  connected,
  pubkey,
  relayUrl,
}: {
  connected: boolean;
  pubkey: string | undefined;
  relayUrl: string | undefined;
}): {
  capability: ChatStateCapability;
  capabilityKey: string | null;
  reprobe: () => void;
} {
  const capabilityKey = chatStateCapabilityKey(relayUrl, pubkey);
  const capability = React.useSyncExternalStore(
    subscribeChatStateCapability,
    () => getChatStateCapability(capabilityKey),
  );
  const lastProbeAtRef = React.useRef(0);

  const reprobe = React.useCallback(() => {
    if (!capabilityKey || !connected) return;
    lastProbeAtRef.current = Date.now();
    void probeChatStateCapability(capabilityKey, () =>
      getChatListPage(null, [], 1),
    );
  }, [capabilityKey, connected]);

  // First probe for this relay + pubkey once connected.
  React.useEffect(() => {
    if (!capabilityKey || !connected) return;
    if (getChatStateCapability(capabilityKey) === "unknown") reprobe();
  }, [capabilityKey, connected, reprobe]);

  React.useEffect(() => {
    if (!capabilityKey) return;
    const unsubscribe = relayClient.subscribeToReconnects(() => {
      // While on, the list query refetch on reconnect is the check.
      if (getChatStateCapability(capabilityKey) !== "on") reprobe();
    });
    const handleFocus = () => {
      if (
        getChatStateCapability(capabilityKey) !== "on" &&
        Date.now() - lastProbeAtRef.current >= CHAT_STATE_FOCUS_PROBE_MS
      ) {
        reprobe();
      }
    };
    window.addEventListener("focus", handleFocus);
    return () => {
      unsubscribe();
      window.removeEventListener("focus", handleFocus);
    };
  }, [capabilityKey, reprobe]);

  return { capability, capabilityKey, reprobe };
}
