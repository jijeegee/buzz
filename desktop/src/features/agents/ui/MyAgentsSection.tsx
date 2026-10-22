import * as React from "react";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import { useAgentWorking } from "@/features/agents/agentWorkingSignal";
import { resolveAgentCardModelLabel } from "@/features/agents/lib/agentCardModelLabel";
import { friendlyAgentLastError } from "@/features/agents/lib/friendlyAgentLastError";
import { isManagedAgentActive } from "@/features/agents/lib/managedAgentControlActions";
import {
  groupRemoteAgentsByDevice,
  localAgentsForList,
} from "@/features/agents/lib/myAgentDevices";
import {
  deleteMyTeam,
  type MyTeam,
  saveMyTeam,
  useMyTeamsLiveUpdates,
  useMyTeamsQuery,
} from "@/features/agents/lib/myTeams";
import {
  type AgentAvailabilityReader,
  useAgentAvailabilityLookup,
} from "@/features/agents/lib/useAgentAvailability";
import { useRemoteOwnedAgents } from "@/features/agents/useRemoteOwnedAgents";
import { useIsArchivedPredicate } from "@/features/identity-archive/hooks";
import { useUserProfileQuery } from "@/features/profile/hooks";
import { useIdentityQuery } from "@/shared/api/hooks";
import { normalizeDeviceId } from "@/shared/api/ownerDevices";
import type { AgentPersona, ManagedAgent } from "@/shared/api/types";
import {
  useAuthDeviceLabels,
  useAuthDevicesQuery,
} from "@/shared/api/useAuthDeviceNames";
import { useOwnerDevices } from "@/shared/api/useOwnerDevices";
import type { ProfilePanelOpenOptions } from "@/shared/context/ProfilePanelContext";
import {
  deviceRobotTag,
  resolveDeviceRobotVariant,
} from "@/shared/lib/deviceRobot";
import { Button } from "@/shared/ui/button";
import { DeviceRobotIcon } from "@/shared/ui/DeviceRobotIcon";
import { Skeleton } from "@/shared/ui/skeleton";
import { AgentCardRobot } from "./AgentCardRobot";
import { StandaloneAgentActionsMenu } from "./AgentStartOnLaunchMenuItem";
import {
  AGENT_TILE_GRID_CLASS,
  AgentTile,
  type AgentTileStatus,
  DeviceHeading,
} from "./AgentTile";
import {
  AddMyTeamToChannelDialog,
  type MyTeamDraft,
  MyTeamDeleteDialog,
  MyTeamDialog,
} from "./MyTeamDialogs";
import { MyTeamsSection, type TeamAgent } from "./MyTeamsSection";
import { RemoteAgentCard } from "./RemoteAgentCard";
import { useT, t } from "@/shared/i18n";

type MyAgentsSectionProps = {
  agents: ManagedAgent[];
  agentsError: Error | null;
  defaultModel: string;
  getAvailability: AgentAvailabilityReader;
  isActionPending: boolean;
  isAgentsLoading: boolean;
  isStartOnLaunchPending: boolean;
  personas: AgentPersona[];
  restartingAgentPubkey: string | null;
  startingAgentPubkey: string | null;
  onAddToChannel: (agent: ManagedAgent) => void;
  onEditPersona: (persona: AgentPersona) => void;
  onOpenAgentProfile: (
    pubkey: string,
    options?: ProfilePanelOpenOptions,
  ) => void;
  onRestartAgent: (pubkey: string) => void;
  onStartAgent: (pubkey: string) => void;
  onStopAgent: (pubkey: string) => void;
  onToggleStartOnLaunch: (agent: ManagedAgent, next: boolean) => void;
};

/** The robot of one of the owner's devices, with their picked override. */
export function DeviceRobot({ deviceId }: { deviceId: string | null }) {
  const { robotOverrides } = useOwnerDevices(deviceId !== null);
  if (!deviceId) return null;
  return (
    <DeviceRobotIcon
      className="h-5 w-5"
      variant={resolveDeviceRobotVariant(
        deviceId,
        robotOverrides.get(normalizeDeviceId(deviceId)),
      )}
    />
  );
}

function agentCountLabel(count: number) {
  return t("agents.myAgents.agentCount", { count });
}

/**
 * The Agents page's "My agents" tab: only real agents (never a preset that
 * has no agent yet), this computer's first, then one group per other
 * computer, each card with its live status and Start/Stop, Edit and Add to
 * channel.
 */
export function MyAgentsSection(props: MyAgentsSectionProps) {
  const t = useT();
  const { agents, agentsError, isAgentsLoading, personas } = props;
  const isArchived = useIsArchivedPredicate();
  const localAgents = React.useMemo(
    () => localAgentsForList(agents, isArchived),
    [agents, isArchived],
  );
  const personaById = React.useMemo(
    () => new Map(personas.map((persona) => [persona.id, persona])),
    [personas],
  );
  const devices = useAuthDevicesQuery();
  const currentDevice = devices.data?.find((device) => device.current) ?? null;
  const { agents: remoteAgents, settling: remoteSettling } =
    useRemoteOwnedAgents();
  const deviceLabels = useAuthDeviceLabels(remoteAgents.length > 0);
  const { getAvailability: getRemoteAvailability } = useAgentAvailabilityLookup(
    React.useMemo(
      () => remoteAgents.map((agent) => agent.pubkey),
      [remoteAgents],
    ),
  );
  const remoteGroups = React.useMemo(
    () =>
      groupRemoteAgentsByDevice(
        remoteAgents.filter((agent) => !isArchived(agent.pubkey)),
        (deviceId) => deviceLabels.get(deviceRobotTag(deviceId) ?? "")?.name,
        // The name the card shows.
        (agent) =>
          agent.name ??
          (agent.localPersonaId
            ? personaById.get(agent.localPersonaId)?.displayName
            : undefined) ??
          "",
      ),
    [deviceLabels, isArchived, personaById, remoteAgents],
  );

  const teamAgents = React.useMemo((): TeamAgent[] => {
    const here = currentDevice?.name
      ? t("agents.myAgents.thisDeviceNamed", { name: currentDevice.name })
      : t("agents.list.thisDevice");
    return [
      ...localAgents.map((agent) => ({
        pubkey: agent.pubkey.toLowerCase(),
        name: agent.name,
        computer: here,
        running: isManagedAgentActive(agent),
      })),
      ...remoteGroups.flatMap((group) =>
        group.agents.map((agent) => {
          const availability = getRemoteAvailability(agent.pubkey);
          return {
            pubkey: agent.pubkey.toLowerCase(),
            name:
              agent.name?.trim() ||
              (agent.localPersonaId
                ? personaById.get(agent.localPersonaId)?.displayName
                : undefined) ||
              t("agents.list.agentFallback"),
            computer: group.name || t("agents.list.anotherDevice"),
            running: availability === "online" || availability === "away",
          };
        }),
      ),
    ];
  }, [
    currentDevice?.name,
    getRemoteAvailability,
    localAgents,
    personaById,
    remoteGroups,
    t,
  ]);

  if (isAgentsLoading) return <TilesSkeleton />;

  return (
    <section className="space-y-6" data-testid="agents-my-agents">
      <div className="space-y-3" data-testid="agents-device-this">
        <DeviceHeading
          detail={[currentDevice?.name, agentCountLabel(localAgents.length)]
            .filter(Boolean)
            .join(" · ")}
          robot={<DeviceRobot deviceId={currentDevice?.id ?? null} />}
          title={t("agents.list.thisDevice")}
        />
        {localAgents.length === 0 ? (
          <p className="text-sm text-muted-foreground">
            {t("agents.myAgents.emptyHere")}
          </p>
        ) : (
          <div className={AGENT_TILE_GRID_CLASS}>
            {localAgents.map((agent) => (
              <LocalAgentTile
                {...props}
                agent={agent}
                key={agent.pubkey}
                persona={
                  agent.personaId
                    ? (personaById.get(agent.personaId) ?? null)
                    : null
                }
              />
            ))}
          </div>
        )}
      </div>

      {remoteSettling && remoteGroups.length === 0 ? (
        <TilesSkeleton />
      ) : (
        remoteGroups.map((group) => (
          <div
            className="space-y-3"
            data-testid={`agents-device-${group.deviceId}`}
            key={group.deviceId}
          >
            <DeviceHeading
              detail={agentCountLabel(group.agents.length)}
              robot={<DeviceRobot deviceId={group.deviceId} />}
              title={group.name || t("agents.list.anotherDevice")}
            />
            <div className={AGENT_TILE_GRID_CLASS}>
              {group.agents.map((agent) => (
                <RemoteAgentCard
                  agent={agent}
                  getAvailability={getRemoteAvailability}
                  hostName={group.name || t("agents.list.anotherDevice")}
                  isPending={props.isActionPending}
                  key={agent.pubkey}
                  persona={
                    agent.localPersonaId
                      ? (personaById.get(agent.localPersonaId) ?? null)
                      : null
                  }
                  onEditPersona={props.onEditPersona}
                  onOpenAgentProfile={props.onOpenAgentProfile}
                />
              ))}
            </div>
          </div>
        ))
      )}

      <MyTeams agents={teamAgents} />

      {agentsError ? (
        <p className="rounded-2xl border border-destructive/30 bg-destructive/10 px-4 py-3 text-sm text-destructive">
          {agentsError.message}
        </p>
      ) : null}
    </section>
  );
}

/** The Teams part of My agents, with its create/edit and channel dialogs. */
function MyTeams({ agents }: { agents: readonly TeamAgent[] }) {
  const t = useT();
  const queryClient = useQueryClient();
  const owner = useIdentityQuery().data?.pubkey ?? "";
  const teamsQuery = useMyTeamsQuery();
  useMyTeamsLiveUpdates();
  const [editing, setEditing] = React.useState<MyTeam | "new" | null>(null);
  const [adding, setAdding] = React.useState<MyTeam | null>(null);
  const [deleting, setDeleting] = React.useState<MyTeam | null>(null);
  const [isSaving, setIsSaving] = React.useState(false);
  const agentsByPubkey = React.useMemo(
    () => new Map(agents.map((agent) => [agent.pubkey, agent])),
    [agents],
  );

  async function save(draft: MyTeamDraft) {
    setIsSaving(true);
    try {
      const team = editing && editing !== "new" ? editing : null;
      await saveMyTeam(queryClient, owner, { id: team?.id, ...draft });
      toast.success(
        team
          ? t("agents.myAgents.teamSaved", { name: draft.name.trim() })
          : t("agents.myAgents.teamCreated"),
      );
    } finally {
      setIsSaving(false);
    }
  }

  async function remove(team: MyTeam) {
    try {
      await deleteMyTeam(queryClient, owner, team);
      toast.success(t("agents.myAgents.teamDeleted", { name: team.name }));
    } catch (error) {
      toast.error(
        error instanceof Error
          ? error.message
          : t("agents.myAgents.teamDeleteFailed"),
      );
    }
  }

  return (
    <>
      <MyTeamsSection
        agentsByPubkey={agentsByPubkey}
        error={teamsQuery.error}
        isLoading={teamsQuery.isLoading}
        teams={teamsQuery.data ?? []}
        onAddToChannel={setAdding}
        onCreate={() => setEditing("new")}
        onDelete={setDeleting}
        onEdit={setEditing}
      />
      <MyTeamDialog
        agents={agents}
        isSaving={isSaving}
        open={editing !== null}
        team={editing === "new" ? null : editing}
        onOpenChange={(open) => {
          if (!open) setEditing(null);
        }}
        onSave={save}
      />
      <MyTeamDeleteDialog
        team={deleting}
        onConfirm={(team) => {
          setDeleting(null);
          void remove(team);
        }}
        onOpenChange={(open) => {
          if (!open) setDeleting(null);
        }}
      />
      {adding ? (
        <AddMyTeamToChannelDialog
          agentsByPubkey={agentsByPubkey}
          team={adding}
          onAdded={(message) => toast.success(message)}
          onOpenChange={(open) => {
            if (!open) setAdding(null);
          }}
        />
      ) : null}
    </>
  );
}

function LocalAgentTile({
  agent,
  defaultModel,
  getAvailability,
  isActionPending,
  isStartOnLaunchPending,
  persona,
  restartingAgentPubkey,
  startingAgentPubkey,
  onAddToChannel,
  onOpenAgentProfile,
  onRestartAgent,
  onStartAgent,
  onStopAgent,
  onToggleStartOnLaunch,
}: MyAgentsSectionProps & {
  agent: ManagedAgent;
  persona: AgentPersona | null;
}) {
  const t = useT();
  const profile = useUserProfileQuery(agent.pubkey).data;
  const working = useAgentWorking(agent.pubkey).working;
  const isActive = isManagedAgentActive(agent);
  const availability = getAvailability(agent.pubkey);
  const friendlyError = isActive
    ? null
    : friendlyAgentLastError(agent.lastError, agent.lastErrorCode)?.copy;
  const status: AgentTileStatus = friendlyError
    ? "error"
    : isActive && working
      ? "working"
      : isActive
        ? "running"
        : "stopped";
  const modelLabel = resolveAgentCardModelLabel({
    agent,
    personaModel: persona?.model,
    provider: persona?.provider ?? agent.provider,
    defaultModel,
  });
  const description = agent.personaOrphaned
    ? t("agents.row.configMissing")
    : (friendlyError ??
      [
        modelLabel,
        persona
          ? t("agents.myAgents.fromPreset", { name: persona.displayName })
          : null,
      ]
        .filter(Boolean)
        .join(" · "));
  const isBusy =
    isActionPending ||
    startingAgentPubkey === agent.pubkey ||
    restartingAgentPubkey === agent.pubkey;
  // Running somewhere else already: starting a second copy is refused.
  const runningElsewhere =
    !isActive && (availability === "online" || availability === "away");

  return (
    <AgentTile
      actions={
        <StandaloneAgentActionsMenu
          agent={agent}
          isPending={isStartOnLaunchPending}
          onToggleStartOnLaunch={onToggleStartOnLaunch}
        />
      }
      avatarUrl={profile?.avatarUrl ?? agent.avatarUrl ?? persona?.avatarUrl}
      buttons={
        <>
          {isActive ? (
            <Button
              data-testid={`agent-tile-stop-${agent.pubkey}`}
              disabled={isBusy}
              onClick={() => onStopAgent(agent.pubkey)}
              size="xs"
              variant="outline"
            >
              {t("agents.list.stop")}
            </Button>
          ) : (
            <Button
              data-testid={`agent-tile-start-${agent.pubkey}`}
              disabled={isBusy || runningElsewhere}
              onClick={() =>
                agent.needsRestart
                  ? onRestartAgent(agent.pubkey)
                  : onStartAgent(agent.pubkey)
              }
              size="xs"
            >
              {startingAgentPubkey === agent.pubkey
                ? t("agents.myAgents.starting")
                : t("agents.runtimeBadge.start")}
            </Button>
          )}
          {isActive && agent.needsRestart ? (
            <Button
              disabled={isBusy}
              onClick={() => onRestartAgent(agent.pubkey)}
              size="xs"
              variant="outline"
            >
              {t("agents.runtimeBadge.restart")}
            </Button>
          ) : null}
          <Button
            data-testid={`agent-tile-edit-${agent.pubkey}`}
            // This agent's own settings; its preset is edited on Presets,
            // since that changes every agent made from it.
            onClick={() => onOpenAgentProfile(agent.pubkey, { tab: "runtime" })}
            size="xs"
            variant="outline"
          >
            {t("common.edit")}
          </Button>
          <Button
            data-testid={`agent-tile-add-to-channel-${agent.pubkey}`}
            onClick={() => onAddToChannel(agent)}
            size="xs"
            variant="outline"
          >
            {t("agents.myAgents.addToGroup")}
          </Button>
        </>
      }
      dataTestId={`managed-agent-${agent.pubkey}`}
      description={description}
      label={agent.name}
      labelBadge={<AgentCardRobot pubkey={agent.pubkey} />}
      openLabel={t("agents.myAgents.openProfile", { name: agent.name })}
      status={status}
      onOpen={() =>
        onOpenAgentProfile(
          agent.pubkey,
          friendlyError ? { tab: "runtime" } : undefined,
        )
      }
    />
  );
}

function TilesSkeleton() {
  return (
    <div className={AGENT_TILE_GRID_CLASS}>
      {[1, 2, 3].map((key) => (
        <Skeleton className="h-32 w-full rounded-xl" key={key} />
      ))}
    </div>
  );
}
