import * as React from "react";
import { useQueryClient } from "@tanstack/react-query";

import { useChannelsQuery } from "@/features/channels/hooks";
import {
  managedAgentsQueryKey,
  useManagedAgentsQuery,
} from "@/features/agents/hooks";
import { relayClient } from "@/shared/api/relayClient";
import { chunkRoomIds } from "@/shared/api/relayRoomBundle";
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
 * so this listens in the owner's DMs with a local agent (all of them on one
 * REQ), like the agent's own harness listens for `!shutdown`. Other signed-in accounts are
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

  // One REQ for every agent DM (per 128, the relay's explicit-channel cap),
  // keyed by its DM set; a changed set opens its successor before the old one
  // closes, and the command ledger acts on a replayed command only once.
  const subsRef = React.useRef(new Map<string, () => Promise<void>>());
  // DMs seen after the first subscription may have just been created by the
  // command itself, so they read back over the command's whole lifetime.
  const startedAtRef = React.useRef<number | null>(null);
  const scopeRef = React.useRef("");
  const wantedRef = React.useRef(new Set<string>());

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
    const wanted = new Set(
      chunkRoomIds(dmKey ? dmKey.split(",") : []).map((chunk) =>
        chunk.join(","),
      ),
    );
    wantedRef.current = wanted;
    // Retire against the latest wanted set: an earlier, slower opening must
    // not close what a later run opened.
    const retire = () => {
      for (const [key, dispose] of subs) {
        if (wantedRef.current.has(key)) continue;
        subs.delete(key);
        void dispose().catch(() => {});
      }
    };

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
    const opening: Promise<void>[] = [];
    for (const key of wanted) {
      if (subs.has(key)) continue;
      const placeholder = async () => {};
      subs.set(key, placeholder);
      const channelIds = key.split(",");
      opening.push(
        relayClient
          .subscribeLive(
            {
              kinds: [KIND_STREAM_MESSAGE],
              authors: [ownerPubkey],
              "#h": channelIds,
              since,
              limit: Math.min(20 * channelIds.length, 500),
            },
            (event) => handleRemoteCommand(event, scope),
          )
          .then((dispose) => {
            if (subs.get(key) === placeholder) {
              subs.set(key, dispose);
            } else {
              void dispose().catch(() => {});
            }
          })
          .catch((error) => {
            if (subs.get(key) === placeholder) subs.delete(key);
            console.error("Failed to listen for remote start", key, error);
          }),
      );
    }
    // The old DM set keeps listening until its successor is set up.
    void Promise.all(opening).then(() => {
      if (scopeRef.current === scopeKey) retire();
    });
  }, [dmKey, ownerPubkey, queryClient, relayUrl]);

  React.useEffect(() => {
    const subs = subsRef.current;
    return () => {
      for (const dispose of subs.values()) void dispose().catch(() => {});
      subs.clear();
    };
  }, []);
}
