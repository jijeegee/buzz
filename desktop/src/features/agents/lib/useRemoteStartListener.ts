import * as React from "react";
import { useQueryClient } from "@tanstack/react-query";

import { useChannelsQuery } from "@/features/channels/hooks";
import {
  managedAgentsQueryKey,
  useManagedAgentsQuery,
} from "@/features/agents/hooks";
import { huddleWindowChannelId } from "@/features/huddle/lib/huddleWindow";
import { relayClient } from "@/shared/api/relayClient";
import { getChannelMembers } from "@/shared/api/tauriChannels";
import type { ChannelMember, ManagedAgent } from "@/shared/api/types";
import { normalizePubkey } from "@/shared/lib/pubkey";
import { agentDmChannelIds } from "./remoteStartCommand";
import { handleRemoteCommand } from "./remoteStartHandler";
import {
  type RemoteStartWatch,
  startRemoteStartWatch,
} from "./remoteStartWatch";

function localAgentPubkeys(agents: readonly ManagedAgent[] | undefined) {
  return (agents ?? [])
    .filter((agent) => agent.backend.type === "local")
    .map((agent) => normalizePubkey(agent.pubkey));
}

/** Whether `members` are exactly the owner and one of `agentPubkeys`. */
export function isOwnerAgentDm(
  members: readonly Pick<ChannelMember, "pubkey">[],
  ownerPubkey: string,
  agentPubkeys: ReadonlySet<string>,
) {
  const owner = normalizePubkey(ownerPubkey);
  const pubkeys = new Set(
    members.map((member) => normalizePubkey(member.pubkey)),
  );
  return (
    pubkeys.size === 2 &&
    pubkeys.has(owner) &&
    [...pubkeys].some((pubkey) => agentPubkeys.has(pubkey))
  );
}

/**
 * Start this computer's agents when the owner asks from another device
 * (desktop Start on a "Running on <PC>" card, or the phone's My agents).
 *
 * Commands are channel messages in the owner's DMs with a local agent, like
 * the agent's own harness listens for `!shutdown`. They arrive on the room
 * bundle (`remoteStartWatch.ts`), hidden agent DMs included; no REQ of its
 * own. The huddle window acts on none: the main window does. Other signed-in
 * accounts are covered by their background watcher
 * (`useInactiveAccountWatch`).
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
  const agentsRef = React.useRef(new Set<string>());
  agentsRef.current = new Set(agentKey ? agentKey.split(",") : []);
  const watchRef = React.useRef<RemoteStartWatch | null>(null);

  React.useEffect(() => {
    if (!ownerPubkey || !relayUrl?.trim()) return;
    const scope = {
      ownerPubkey,
      relayUrl,
      openAccount: true,
      onStarted: () => {
        void queryClient.invalidateQueries({ queryKey: managedAgentsQueryKey });
      },
    };
    // A hidden DM's members, read once per room; checked against the agents
    // at the time of each command.
    const members = new Map<string, Promise<readonly ChannelMember[]>>();
    const rooms = relayClient.liveRooms;
    const watch = startRemoteStartWatch({
      ownerPubkey,
      inHuddleWindow: huddleWindowChannelId() !== null,
      listenAllRooms: (listener) => rooms.listenAllRooms(listener),
      listenQuietRooms: (listener) => rooms.listenQuietRooms(listener),
      whenRoomsCovered: (roomIds) => rooms.whenRoomsCovered(roomIds),
      fetchEvents: (filter) => relayClient.fetchEvents(filter),
      isAgentDm: async (roomId) => {
        let read = members.get(roomId);
        if (!read) {
          read = getChannelMembers(roomId);
          members.set(roomId, read);
          read.catch(() => members.delete(roomId));
        }
        return isOwnerAgentDm(await read, ownerPubkey, agentsRef.current);
      },
      handle: (event) => handleRemoteCommand(event, scope),
      nowSeconds: () => Math.floor(Date.now() / 1_000),
    });
    watchRef.current = watch;
    return () => {
      watch.stop();
      if (watchRef.current === watch) watchRef.current = null;
    };
  }, [ownerPubkey, queryClient, relayUrl]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: a new watch (account or community) gets the current set again
  React.useEffect(() => {
    watchRef.current?.setWanted(dmKey ? dmKey.split(",") : []);
  }, [dmKey, ownerPubkey, relayUrl]);
}
