import * as React from "react";
import { useQueryClient } from "@tanstack/react-query";

import { useChannelsQuery } from "@/features/channels/hooks";
import {
  managedAgentsQueryKey,
  useManagedAgentsQuery,
} from "@/features/agents/hooks";
import { relayClient } from "@/shared/api/relayClient";
import type { ManagedAgent } from "@/shared/api/types";
import { KIND_STREAM_MESSAGE } from "@/shared/constants/kinds";
import { normalizePubkey } from "@/shared/lib/pubkey";
import {
  agentDmChannelIds,
  REMOTE_START_MAX_AGE_SECONDS,
} from "./remoteStartCommand";
import { handleRemoteCommand } from "./remoteStartHandler";

function localAgentPubkeys(agents: readonly ManagedAgent[] | undefined) {
  return (agents ?? [])
    .filter((agent) => agent.backend.type === "local")
    .map((agent) => normalizePubkey(agent.pubkey));
}

/**
 * Start this computer's agents when the owner asks from another device
 * (desktop Start on a "Running on <PC>" card, or the phone's My agents).
 *
 * The relay only delivers channel messages to channel-scoped subscriptions,
 * so this listens in each of the owner's DMs with a local agent, like the
 * agent's own harness listens for `!shutdown`. Other signed-in accounts are
 * covered by their background watcher (`useInactiveAccountWatch`).
 */
export function useRemoteStartListener(
  ownerPubkey: string | undefined,
  relayUrl: string | undefined,
) {
  const queryClient = useQueryClient();
  const channelsQuery = useChannelsQuery({ enabled: Boolean(ownerPubkey) });
  const agentsQuery = useManagedAgentsQuery({
    enabled: Boolean(ownerPubkey),
  });
  const agentKey = localAgentPubkeys(agentsQuery.data).sort().join(",");
  const dmKey = React.useMemo(
    () =>
      ownerPubkey && agentKey
        ? agentDmChannelIds(
            channelsQuery.data ?? [],
            agentKey.split(","),
            ownerPubkey,
          ).join(",")
        : "",
    [agentKey, channelsQuery.data, ownerPubkey],
  );

  const subsRef = React.useRef(new Map<string, () => Promise<void>>());
  // DMs seen after the first subscription may have just been created by the
  // command itself, so they read back over the command's whole lifetime.
  const startedAtRef = React.useRef<number | null>(null);
  const scopeRef = React.useRef("");

  React.useEffect(() => {
    const subs = subsRef.current;
    // A new account or community starts over.
    const scopeKey = `${ownerPubkey ?? ""}|${relayUrl ?? ""}`;
    if (scopeRef.current !== scopeKey) {
      scopeRef.current = scopeKey;
      for (const dispose of subs.values()) void dispose().catch(() => {});
      subs.clear();
      startedAtRef.current = null;
    }
    if (!ownerPubkey || !relayUrl?.trim()) return;
    const wanted = new Set(dmKey ? dmKey.split(",") : []);
    for (const [channelId, dispose] of subs) {
      if (wanted.has(channelId)) continue;
      subs.delete(channelId);
      void dispose().catch(() => {});
    }

    const nowSeconds = Math.floor(Date.now() / 1_000);
    const since =
      startedAtRef.current === null
        ? nowSeconds
        : nowSeconds - REMOTE_START_MAX_AGE_SECONDS;
    if (wanted.size > 0) startedAtRef.current ??= nowSeconds;

    const scope = {
      ownerPubkey,
      relayUrl,
      openAccount: true,
      onStarted: () => {
        void queryClient.invalidateQueries({ queryKey: managedAgentsQueryKey });
      },
    };
    for (const channelId of wanted) {
      if (subs.has(channelId)) continue;
      const placeholder = async () => {};
      subs.set(channelId, placeholder);
      void relayClient
        .subscribeLive(
          {
            kinds: [KIND_STREAM_MESSAGE],
            authors: [ownerPubkey],
            "#h": [channelId],
            since,
            limit: 20,
          },
          (event) => handleRemoteCommand(event, scope),
        )
        .then((dispose) => {
          if (subs.get(channelId) === placeholder) {
            subs.set(channelId, dispose);
          } else {
            void dispose().catch(() => {});
          }
        })
        .catch((error) => {
          if (subs.get(channelId) === placeholder) subs.delete(channelId);
          console.error("Failed to listen for remote start", channelId, error);
        });
    }
  }, [dmKey, ownerPubkey, queryClient, relayUrl]);

  React.useEffect(() => {
    const subs = subsRef.current;
    return () => {
      for (const dispose of subs.values()) void dispose().catch(() => {});
      subs.clear();
    };
  }, []);
}
