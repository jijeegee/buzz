import * as React from "react";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import { managedAgentsQueryKey } from "@/features/agents/hooks";
import { relayClient } from "@/shared/api/relayClient";
import { listManagedAgents } from "@/shared/api/tauri";
import { startManagedAgent } from "@/shared/api/tauriManagedAgents";
import {
  KIND_STREAM_MESSAGE,
  KIND_STREAM_MESSAGE_V2,
} from "@/shared/constants/kinds";
import { normalizePubkey } from "@/shared/lib/pubkey";
import { remoteStartTargets } from "./remoteStartCommand";

/**
 * Start this computer's agents when the owner asks from another device
 * (desktop Start on a "Running on <PC>" card, or the phone's My agents).
 * Listens to the owner's own channel messages from now on and starts each
 * stopped local agent a fresh `!start` mentions.
 */
export function useRemoteStartListener(
  ownerPubkey: string | undefined,
  relayUrl: string | undefined,
) {
  const queryClient = useQueryClient();

  React.useEffect(() => {
    if (!ownerPubkey || !relayUrl?.trim()) return;
    let cancelled = false;
    let dispose: (() => Promise<void>) | null = null;
    const seen = new Set<string>();

    void relayClient
      .subscribeLive(
        {
          authors: [ownerPubkey],
          kinds: [KIND_STREAM_MESSAGE, KIND_STREAM_MESSAGE_V2],
          since: Math.floor(Date.now() / 1_000),
          limit: 50,
        },
        (event) => {
          if (seen.has(event.id)) return;
          seen.add(event.id);
          if (!event.content.trim().startsWith("!")) return;
          void (async () => {
            const targets = remoteStartTargets(event, {
              localAgents: await listManagedAgents(),
              nowSeconds: Math.floor(Date.now() / 1_000),
              ownerPubkey,
            });
            for (const pubkey of targets) {
              if (cancelled) return;
              try {
                // Scoped like a mention wake: a community or account
                // switch since the command arrived fails closed.
                const agent = await startManagedAgent(pubkey, {
                  expectedRelayUrl: relayUrl,
                  expectedSignerPubkey: normalizePubkey(ownerPubkey),
                });
                toast.success(`${agent.name} started from another device.`);
              } catch (error) {
                console.error("Remote start failed", pubkey, error);
              }
            }
            if (targets.length > 0) {
              void queryClient.invalidateQueries({
                queryKey: managedAgentsQueryKey,
              });
            }
          })().catch((error) => {
            console.error("Remote start check failed", error);
          });
        },
      )
      .then((disposer) => {
        if (cancelled) {
          void disposer();
        } else {
          dispose = disposer;
        }
      })
      .catch((error) => {
        console.error("Failed to listen for remote start", error);
      });

    return () => {
      cancelled = true;
      if (dispose) void dispose();
    };
  }, [ownerPubkey, queryClient, relayUrl]);
}
