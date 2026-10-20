import * as React from "react";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import { managedAgentsQueryKey } from "@/features/agents/hooks";
import { sendAgentObserverControl } from "@/shared/api/observerRelay";
import { listAllManagedAgents } from "@/shared/api/tauri";
import {
  startManagedAgent,
  stopManagedAgent,
} from "@/shared/api/tauriManagedAgents";
import { listAuthDevices } from "@/shared/api/tokenAuth";
import { subscribeAgentControlRequests } from "../observerRelayStore";
import { handleAgentControlRequest } from "./agentControlHandler";

let computerNamePromise: Promise<string> | null = null;

/** This computer's name as the owner's device list shows it. */
function currentComputerName(): Promise<string> {
  computerNamePromise ??= listAuthDevices()
    .then(
      (devices) =>
        devices.find((device) => device.current)?.name.trim() ||
        "this computer",
    )
    .catch(() => {
      computerNamePromise = null;
      return "this computer";
    });
  return computerNamePromise;
}

/**
 * Answers `kuzz agents list|start|stop|restart` from the open account's
 * agents: lists this computer's agents and starts, stops or restarts the one
 * named when it runs here. Requests arrive on the owner's observer
 * subscription, which only carries frames from the owner's own agents.
 */
export function useAgentControlRequests(
  ownerPubkey: string | undefined,
  relayUrl: string | undefined,
) {
  const queryClient = useQueryClient();

  React.useEffect(() => {
    if (!ownerPubkey || !relayUrl?.trim()) return;
    const scope = { ownerPubkey, relayUrl };
    return subscribeAgentControlRequests((agentPubkey, request, sentAt) => {
      handleAgentControlRequest(agentPubkey, request, sentAt, scope, {
        listAgents: listAllManagedAgents,
        start: startManagedAgent,
        stop: stopManagedAgent,
        computerName: currentComputerName,
        reply: sendAgentObserverControl,
        notify: (message) => toast.info(message),
        now: Date.now,
      })
        .catch((error) => {
          console.error("Agent control request failed", request, error);
        })
        .finally(() => {
          if (request.action !== "list") {
            void queryClient.invalidateQueries({
              queryKey: managedAgentsQueryKey,
            });
          }
        });
    });
  }, [ownerPubkey, queryClient, relayUrl]);
}
