import { isRelayRemovedError } from "@/features/agents/managedAgentRelayCleanup";
import { useCommunities } from "@/features/communities/useCommunities";
import * as React from "react";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import { agentsOfAccount } from "@/features/accounts/lib/accountLabels";
import { freeAgentName } from "@/shared/api/agentNameGuard";
import { useIdentityQuery } from "@/shared/api/hooks";

import {
  type AttachManagedAgentToChannelResult,
  useAvailableAcpRuntimes,
  useCreateManagedAgentMutation,
  useManagedAgentLogQuery,
  useManagedAgentsQuery,
  useRelayAgentsQuery,
  useSetManagedAgentStartOnAppLaunchMutation,
  useStartManagedAgentMutation,
  useStopManagedAgentMutation,
  useDeleteManagedAgentMutation,
} from "@/features/agents/hooks";
import {
  agentPresenceStartBlockReason,
  useAgentAvailabilityLookup,
} from "../lib/useAgentAvailability";
import { useGlobalAgentConfig } from "@/features/agents/useGlobalAgentConfig";
import { useChannelsQuery } from "@/features/channels/hooks";
import { invalidateChannelMembersRosters } from "@/features/channels/rosterFreshness";
import { type MyTeam, saveMyTeam } from "@/features/agents/lib/myTeams";
import { resolveTeamPersonas } from "@/features/agents/lib/teamPersonas";
import type {
  AgentPersona,
  AgentTeam,
  Channel,
  ManagedAgent,
} from "@/shared/api/types";
import { removeChannelMember } from "@/shared/api/tauri";
import { normalizePubkey } from "@/shared/lib/pubkey";
import {
  deleteManagedAgentWithRules,
  isManagedAgentActive,
  respawnManagedAgentWithRules,
  startManagedAgentWithRules,
  stopManagedAgentWithRules,
} from "../lib/managedAgentControlActions";
import { clearActiveTurnsForAgentOnStop } from "../managedAgentRuntimeHooks";
import {
  availableRuntimesForStart,
  buildInstanceInputForDefinition,
  resolveStartRuntimeForDefinition,
} from "../lib/instanceInputForDefinition";
import { useT } from "@/shared/i18n";

export function useManagedAgentActions() {
  const t = useT();
  const queryClient = useQueryClient();
  const relayUrl = useCommunities().activeCommunity?.relayUrl;
  const { globalConfig } = useGlobalAgentConfig();
  const relayAgentsQuery = useRelayAgentsQuery();
  const managedAgentsQuery = useManagedAgentsQuery();
  const activeAccount = useIdentityQuery().data?.pubkey;
  const [settingUpTeamId, setSettingUpTeamId] = React.useState<string | null>(
    null,
  );
  const [shouldLoadChannels, setShouldLoadChannels] = React.useState(false);
  const channelsQuery = useChannelsQuery({ enabled: shouldLoadChannels });
  const startMutation = useStartManagedAgentMutation();
  const stopMutation = useStopManagedAgentMutation();
  const deleteMutation = useDeleteManagedAgentMutation();
  const createAgentMutation = useCreateManagedAgentMutation();
  const availableRuntimesQuery = useAvailableAcpRuntimes();
  const startOnLaunchMutation = useSetManagedAgentStartOnAppLaunchMutation();
  const [isCreateOpen, setIsCreateOpen] = React.useState(false);
  const [agentToAddToChannel, setAgentToAddToChannel] =
    React.useState<ManagedAgent | null>(null);
  const [startingPersonaIds, setStartingPersonaIds] = React.useState<
    ReadonlySet<string>
  >(() => new Set());
  const startingPersonaIdsRef = React.useRef(new Set<string>());
  const [restartingAgentPubkey, setRestartingAgentPubkey] = React.useState<
    string | null
  >(null);
  const [logAgentPubkey, setLogAgentPubkey] = React.useState<string | null>(
    null,
  );
  const [actionNoticeMessage, setActionNoticeMessage] = React.useState<
    string | null
  >(null);
  const [actionErrorMessage, setActionErrorMessage] = React.useState<
    string | null
  >(null);

  const managedAgentLogQuery = useManagedAgentLogQuery(logAgentPubkey);

  React.useEffect(() => {
    const timeoutId = window.setTimeout(() => {
      setShouldLoadChannels(true);
    }, 0);
    return () => window.clearTimeout(timeoutId);
  }, []);

  const managedAgents = React.useMemo(
    () =>
      // Each account manages its own agents (others keep running).
      agentsOfAccount(managedAgentsQuery.data ?? [], activeAccount).sort(
        (left, right) => {
          const activeScore = (s: string) =>
            s === "running" || s === "deployed" ? 1 : 0;
          const diff = activeScore(right.status) - activeScore(left.status);
          if (diff !== 0) return diff;
          return left.name.localeCompare(right.name);
        },
      ),
    [managedAgentsQuery.data, activeAccount],
  );
  // Observer ingestion is owner-global (useAgentObserverIngestion in
  // AppShell); this hook only reads derived state.

  const managedPubkeys = React.useMemo(
    () => new Set(managedAgents.map((agent) => agent.pubkey)),
    [managedAgents],
  );

  const managedPubkeyList = React.useMemo(
    () => managedAgents.map((agent) => agent.pubkey),
    [managedAgents],
  );

  const { query: managedPresenceQuery, getAvailability } =
    useAgentAvailabilityLookup(managedPubkeyList);

  function assertStartNotBlockedByPresence(agent: ManagedAgent) {
    const reason = agentPresenceStartBlockReason(
      isManagedAgentActive(agent),
      getAvailability(agent.pubkey),
    );
    if (reason) throw new Error(reason);
  }

  const channelsByPubkey = React.useMemo(() => {
    const map: Record<string, { id: string; name: string }[]> = {};
    // Seed from relay agent profiles (kind:10100 events).
    for (const ra of relayAgentsQuery.data ?? []) {
      if (ra.channels.length > 0) {
        // Skip entries missing a channel id rather than falling back to the
        // name as id — a misaligned channels/channelIds pairing would otherwise
        // produce a pill that silently navigates to a channel name as if it
        // were an id.
        map[normalizePubkey(ra.pubkey)] = ra.channels.flatMap((name, i) => {
          const id = ra.channelIds[i];
          return id ? [{ id, name }] : [];
        });
      }
    }
    // Fill in from channel member lists (kind:39002) for any managed agents
    // not already covered by relay agent data.
    const normalizedManaged = new Set(
      managedAgents.map((a) => normalizePubkey(a.pubkey)),
    );
    for (const ch of channelsQuery.data ?? []) {
      for (const pk of ch.memberPubkeys) {
        const key = normalizePubkey(pk);
        if (!normalizedManaged.has(key)) continue;
        if (!map[key]) map[key] = [];
        if (!map[key].some((entry) => entry.id === ch.id)) {
          map[key].push({ id: ch.id, name: ch.name });
        }
      }
    }
    return map;
  }, [relayAgentsQuery.data, channelsQuery.data, managedAgents]);

  const channelIdToName = React.useMemo(() => {
    const map: Record<string, string> = {};
    for (const ch of channelsQuery.data ?? []) {
      map[ch.id] = ch.name;
    }
    return map;
  }, [channelsQuery.data]);

  // Clear log selection if the agent was removed
  React.useEffect(() => {
    if (
      logAgentPubkey &&
      !managedAgents.some((agent) => agent.pubkey === logAgentPubkey)
    ) {
      setLogAgentPubkey(null);
    }
  }, [managedAgents, logAgentPubkey]);

  function clearFeedback() {
    setActionNoticeMessage(null);
    setActionErrorMessage(null);
  }

  async function handleStart(pubkey: string) {
    clearFeedback();
    try {
      const agent = managedAgents.find((c) => c.pubkey === pubkey);
      if (!agent) return;
      assertStartNotBlockedByPresence(agent);
      await startManagedAgentWithRules({
        agent,
        startManagedAgent: startMutation.mutateAsync,
      });
    } catch (error) {
      if (isRelayRemovedError(error)) return;
      setActionErrorMessage(
        error instanceof Error
          ? error.message
          : t("agents.actions.startFailed"),
      );
    }
  }

  async function handleRestart(pubkey: string) {
    if (restartingAgentPubkey) return;
    clearFeedback();
    setRestartingAgentPubkey(pubkey);
    try {
      const agent = managedAgents.find(
        (candidate) => candidate.pubkey === pubkey,
      );
      if (!agent) return;
      assertStartNotBlockedByPresence(agent);
      await respawnManagedAgentWithRules({
        agent,
        relayUrl,
        startManagedAgent: startMutation.mutateAsync,
        stopManagedAgent: stopMutation.mutateAsync,
        onStopped: () => clearActiveTurnsForAgentOnStop(agent.pubkey),
      });
    } catch (error) {
      if (isRelayRemovedError(error)) return;
      setActionErrorMessage(
        error instanceof Error
          ? error.message
          : t("agents.actions.restartFailed"),
      );
    } finally {
      setRestartingAgentPubkey(null);
    }
  }

  function setPersonaStartPending(personaId: string, pending: boolean) {
    const next = new Set(startingPersonaIdsRef.current);
    if (pending) {
      next.add(personaId);
    } else {
      next.delete(personaId);
    }
    startingPersonaIdsRef.current = next;
    setStartingPersonaIds(next);
  }

  /**
   * Make one new agent on this computer from a preset, named after it
   * ("Fizz", then "Fizz 2", …). Spawn problems and notices go to the page
   * feedback; a failed create throws.
   */
  async function createAgentFromPreset(persona: AgentPersona) {
    const runtimes = await availableRuntimesForStart(availableRuntimesQuery);
    const { runtime, warnings } = resolveStartRuntimeForDefinition(
      persona,
      runtimes,
      globalConfig.preferred_runtime,
    );
    const input = await buildInstanceInputForDefinition(persona, runtime);
    // A preset can make several agents: the next one is "Fizz 2".
    input.name = await freeAgentName({
      kind: "agent",
      name: input.name,
      personaId: persona.id,
    });

    const created = await createAgentMutation.mutateAsync(input);
    const notices = [...warnings];
    if (created.spawnError && !isRelayRemovedError(created.spawnError)) {
      setActionErrorMessage(created.spawnError);
    }
    if (created.profileSyncError) {
      notices.push(created.profileSyncError);
    }
    if (notices.length > 0) {
      setActionNoticeMessage(notices.join(" "));
    }
    return created.agent;
  }

  async function handleStartPersona(persona: AgentPersona) {
    if (startingPersonaIdsRef.current.has(persona.id)) {
      return;
    }
    setPersonaStartPending(persona.id, true);
    clearFeedback();
    try {
      await createAgentFromPreset(persona);
      toast.success(t("agents.actions.created"));
      void managedAgentsQuery.refetch();
      void relayAgentsQuery.refetch();
    } catch (error) {
      if (isRelayRemovedError(error)) return;
      setActionErrorMessage(
        error instanceof Error
          ? error.message
          : t("agents.actions.startFailed"),
      );
    } finally {
      setPersonaStartPending(persona.id, false);
    }
  }

  /**
   * "Set up on… This computer" for a team preset: make one agent here per
   * preset in it, then a team of those agents named after the preset.
   * Returns the new team, or `null` when nothing could be made.
   */
  async function handleSetUpTeamPreset(
    team: AgentTeam,
    personas: AgentPersona[],
  ): Promise<MyTeam | null> {
    if (settingUpTeamId) return null;
    setSettingUpTeamId(team.id);
    clearFeedback();
    const made: string[] = [];
    const failed: string[] = [];
    try {
      const { resolvedPersonas } = resolveTeamPersonas(team, personas);
      for (const persona of resolvedPersonas) {
        try {
          const agent = await createAgentFromPreset(persona);
          made.push(normalizePubkey(agent.pubkey));
        } catch (error) {
          if (isRelayRemovedError(error)) continue;
          failed.push(
            error instanceof Error
              ? `${persona.displayName}: ${error.message}`
              : t("agents.actions.couldNotBeMade", {
                  name: persona.displayName,
                }),
          );
        }
      }
      if (made.length === 0) {
        setActionErrorMessage(
          failed.length > 0
            ? t("agents.actions.setUpFailedWithDetails", {
                name: team.name,
                details: failed.join(" "),
              })
            : t("agents.actions.noPresets", { name: team.name }),
        );
        return null;
      }
      const saved = await saveMyTeam(queryClient, activeAccount ?? "", {
        name: team.name,
        description: team.description,
        agentPubkeys: made,
      });
      toast.success(
        t("agents.actions.setUpDone", { name: team.name, count: made.length }),
      );
      if (failed.length > 0) {
        setActionErrorMessage(
          t("agents.actions.someNotMade", { details: failed.join(" ") }),
        );
      }
      return saved;
    } catch (error) {
      setActionErrorMessage(
        error instanceof Error
          ? error.message
          : t("agents.actions.setUpFailed", { name: team.name }),
      );
      return null;
    } finally {
      void managedAgentsQuery.refetch();
      void relayAgentsQuery.refetch();
      setSettingUpTeamId(null);
    }
  }

  async function getChannelsForAction() {
    if (channelsQuery.data) {
      return channelsQuery.data;
    }

    const result = await channelsQuery.refetch();
    return result.data ?? [];
  }

  async function handleStop(pubkey: string) {
    clearFeedback();
    try {
      const agent = managedAgents.find((a) => a.pubkey === pubkey);
      if (!agent) return;
      const channels = await getChannelsForAction();
      const result = await stopManagedAgentWithRules({
        agent,
        channels,
        relayAgents: relayAgentsQuery.data ?? [],
        stopManagedAgent: stopMutation.mutateAsync,
      });
      if (agent.backend.type === "local") {
        clearActiveTurnsForAgentOnStop(pubkey);
      }
      if (result.noticeMessage) {
        setActionNoticeMessage(result.noticeMessage);
      }
    } catch (error) {
      setActionErrorMessage(
        error instanceof Error ? error.message : t("agents.actions.stopFailed"),
      );
    }
  }

  function getAgentChannelIds(pubkey: string): string[] {
    const normalized = normalizePubkey(pubkey);
    const relayAgent = (relayAgentsQuery.data ?? []).find(
      (ra) => normalizePubkey(ra.pubkey) === normalized,
    );
    return relayAgent?.channelIds ?? [];
  }

  async function removeAgentFromAllChannels(pubkey: string) {
    const channelIds = getAgentChannelIds(pubkey);
    if (channelIds.length === 0) return;
    await Promise.allSettled(
      channelIds.map((channelId) => removeChannelMember(channelId, pubkey)),
    );
    // Direct writes bypass the member mutations' invalidation; without this,
    // the deleted agent stays in cached rosters for the freshness window.
    await invalidateChannelMembersRosters(queryClient, channelIds);
  }

  async function handleDelete(pubkey: string) {
    clearFeedback();
    try {
      const agent = managedAgents.find((a) => a.pubkey === pubkey);
      if (!agent) return;
      const channels = await getChannelsForAction();
      const result = await deleteManagedAgentWithRules({
        agent,
        channels,
        deleteManagedAgent: deleteMutation.mutateAsync,
        getAvailability,
        relayAgents: relayAgentsQuery.data ?? [],
      });
      if (result.cancelled) return;
      await removeAgentFromAllChannels(pubkey);
      if (logAgentPubkey === pubkey) {
        setLogAgentPubkey(null);
      }
    } catch (error) {
      setActionErrorMessage(
        error instanceof Error
          ? error.message
          : t("agents.actions.deleteFailed"),
      );
    }
  }

  async function handleToggleStartOnAppLaunch(
    pubkey: string,
    startOnAppLaunch: boolean,
  ) {
    clearFeedback();
    try {
      const updated = await startOnLaunchMutation.mutateAsync({
        pubkey,
        startOnAppLaunch,
      });
      setActionNoticeMessage(
        updated.startOnAppLaunch
          ? t("agents.actions.startOnLaunchOn", { name: updated.name })
          : t("agents.actions.startOnLaunchOff", { name: updated.name }),
      );
    } catch (error) {
      setActionErrorMessage(
        error instanceof Error
          ? error.message
          : t("agents.actions.startOnLaunchFailed"),
      );
    }
  }

  function handleAddedToChannel(
    channel: Channel,
    result: AttachManagedAgentToChannelResult,
  ) {
    setActionErrorMessage(null);
    setActionNoticeMessage(() => {
      if (result.started) {
        return t("agents.actions.addedAndStarted", {
          name: result.agent.name,
          group: channel.name,
        });
      }
      if (result.membershipAdded) {
        return t("agents.actions.addedToGroup", {
          name: result.agent.name,
          group: channel.name,
        });
      }
      return t("agents.actions.alreadyInGroup", {
        name: result.agent.name,
        group: channel.name,
      });
    });
    void managedAgentsQuery.refetch();
    void relayAgentsQuery.refetch();
  }

  async function runBulkAction(
    targets: ManagedAgent[],
    confirmMessage: (count: number) => string,
    failureMessage: (failed: number, total: number) => string,
    action: (agent: ManagedAgent) => Promise<unknown>,
  ): Promise<boolean> {
    if (targets.length === 0) return false;
    const confirmed = window.confirm(confirmMessage(targets.length));
    if (!confirmed) return false;
    clearFeedback();
    const results = await Promise.allSettled(targets.map(action));
    const failures = results.filter((r) => r.status === "rejected");
    if (failures.length > 0) {
      setActionErrorMessage(failureMessage(failures.length, targets.length));
    }
    return true;
  }

  async function handleBulkStopRunning() {
    await runBulkAction(
      managedAgents.filter((a) => isManagedAgentActive(a)),
      (count) => t("agents.actions.bulkStopConfirm", { count }),
      (failed, total) => t("agents.actions.bulkStopFailed", { failed, total }),
      async (a) => {
        await stopManagedAgentWithRules({
          agent: a,
          channels: channelsQuery.data ?? [],
          relayAgents: relayAgentsQuery.data ?? [],
          stopManagedAgent: stopMutation.mutateAsync,
        });
        if (a.backend.type === "local") {
          clearActiveTurnsForAgentOnStop(a.pubkey);
        }
      },
    );
  }

  const isPending =
    restartingAgentPubkey !== null ||
    createAgentMutation.isPending ||
    startMutation.isPending ||
    stopMutation.isPending ||
    deleteMutation.isPending;
  const startingAgentPubkey =
    startMutation.isPending && typeof startMutation.variables === "string"
      ? startMutation.variables
      : null;

  return {
    relayAgentsQuery,
    managedAgentsQuery,
    managedAgentLogQuery,
    managedPresenceQuery,
    getAvailability,
    managedAgents,
    managedPubkeys,
    channelIdToName,
    channelsByPubkey,
    isPending,
    /** The start-on-launch mutation alone; the card menu item locks on this, not `isPending`. */
    isStartOnLaunchPending: startOnLaunchMutation.isPending,
    isCreateOpen,
    setIsCreateOpen,
    agentToAddToChannel,
    setAgentToAddToChannel,
    logAgentPubkey,
    setLogAgentPubkey,
    actionNoticeMessage,
    setActionNoticeMessage,
    actionErrorMessage,
    setActionErrorMessage,
    startingAgentPubkey,
    restartingAgentPubkey,
    startingPersonaIds,
    settingUpTeamId,
    handleSetUpTeamPreset,
    handleStart,
    handleRestart,
    handleStartPersona,
    handleStop,
    handleDelete,
    handleToggleStartOnAppLaunch,
    handleAddedToChannel,
    handleBulkStopRunning,
    refetchManagedAgents: () => void managedAgentsQuery.refetch(),
    refetchRelayAgents: () => void relayAgentsQuery.refetch(),
  };
}
