import * as React from "react";

import { useAgentWorking } from "@/features/agents/agentWorkingSignal";
import { resolveAgentCardModelLabel } from "@/features/agents/lib/agentCardModelLabel";
import { friendlyAgentLastError } from "@/features/agents/lib/friendlyAgentLastError";
import { isManagedAgentActive } from "@/features/agents/lib/managedAgentControlActions";
import {
  groupRemoteAgentsByDevice,
  localAgentsForList,
} from "@/features/agents/lib/myAgentDevices";
import {
  type AgentAvailabilityReader,
  useAgentAvailabilityLookup,
} from "@/features/agents/lib/useAgentAvailability";
import { useRemoteOwnedAgents } from "@/features/agents/useRemoteOwnedAgents";
import { useIsArchivedPredicate } from "@/features/identity-archive/hooks";
import { useUserProfileQuery } from "@/features/profile/hooks";
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
import { RemoteAgentCard } from "./RemoteAgentCard";

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
  return count === 1 ? "1 agent" : `${count} agents`;
}

/**
 * The Agents page's "My agents" tab: only real agents (never a preset that
 * has no agent yet), this computer's first, then one group per other
 * computer, each card with its live status and Start/Stop, Edit and Add to
 * channel.
 */
export function MyAgentsSection(props: MyAgentsSectionProps) {
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

  if (isAgentsLoading) return <TilesSkeleton />;

  return (
    <section className="space-y-6" data-testid="agents-my-agents">
      <div className="space-y-3" data-testid="agents-device-this">
        <DeviceHeading
          detail={[currentDevice?.name, agentCountLabel(localAgents.length)]
            .filter(Boolean)
            .join(" · ")}
          robot={<DeviceRobot deviceId={currentDevice?.id ?? null} />}
          title="This device"
        />
        {localAgents.length === 0 ? (
          <p className="text-sm text-muted-foreground">
            No agents on this computer yet. Use New agent, or Create agent on a
            preset.
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
              title={group.name || "Another device"}
            />
            <div className={AGENT_TILE_GRID_CLASS}>
              {group.agents.map((agent) => (
                <RemoteAgentCard
                  agent={agent}
                  getAvailability={getRemoteAvailability}
                  hostName={group.name || "Another device"}
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

      {agentsError ? (
        <p className="rounded-2xl border border-destructive/30 bg-destructive/10 px-4 py-3 text-sm text-destructive">
          {agentsError.message}
        </p>
      ) : null}
    </section>
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
    ? "Configuration missing"
    : (friendlyError ??
      [modelLabel, persona ? `from preset ${persona.displayName}` : null]
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
              Stop
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
              {startingAgentPubkey === agent.pubkey ? "Starting…" : "Start"}
            </Button>
          )}
          {isActive && agent.needsRestart ? (
            <Button
              disabled={isBusy}
              onClick={() => onRestartAgent(agent.pubkey)}
              size="xs"
              variant="outline"
            >
              Restart
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
            Edit
          </Button>
          <Button
            data-testid={`agent-tile-add-to-channel-${agent.pubkey}`}
            onClick={() => onAddToChannel(agent)}
            size="xs"
            variant="outline"
          >
            Add to channel
          </Button>
        </>
      }
      dataTestId={`managed-agent-${agent.pubkey}`}
      description={description}
      label={agent.name}
      labelBadge={<AgentCardRobot pubkey={agent.pubkey} />}
      openLabel={`${agent.name} agent profile`}
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
