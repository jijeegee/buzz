import * as React from "react";
import { CopyPlus, EllipsisVertical, Pencil, Play, Square } from "lucide-react";
import { toast } from "sonner";

import { useRelayAgentsQuery } from "@/features/agents/hooks";
import {
  type ManagedAgentActionResult,
  startRemoteOwnedAgent,
  stopRemoteOwnedAgent,
} from "@/features/agents/lib/managedAgentControlActions";
import type { AgentAvailabilityReader } from "@/features/agents/lib/useAgentAvailability";
import type { RemoteOwnedAgentHere } from "@/features/agents/useRemoteOwnedAgents";
import { useChannelsQuery } from "@/features/channels/hooks";
import { useUserProfileQuery } from "@/features/profile/hooks";
import { agentHostDeviceLabel } from "@/features/profile/ui/AgentBadgeIcon";
import type { AgentPersona, Channel, RelayAgent } from "@/shared/api/types";
import { useAuthDeviceLabels } from "@/shared/api/useAuthDeviceNames";
import { deviceRobotTag } from "@/shared/lib/deviceRobot";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/shared/ui/dropdown-menu";
import { AgentIdentityCard } from "./AgentIdentityCard";
import { AgentRuntimeAvatarControl } from "./AgentRuntimeAvatarControl";

/**
 * One of the owner's agents that runs on another of their computers. It is
 * the same agent everywhere, so this computer shows it with that computer's
 * name and availability. It has no key for the agent, so Start asks the
 * hosting computer's app to start it and Stop asks the agent itself to exit;
 * edits to its persona reach the hosting computer through persona sync.
 */
export function RemoteAgentCard({
  agent,
  getAvailability,
  isPending,
  persona,
  onDuplicatePersona,
  onEditPersona,
  onOpenAgentProfile,
}: {
  agent: RemoteOwnedAgentHere;
  getAvailability: AgentAvailabilityReader;
  isPending: boolean;
  /** This computer's persona the agent was made from, if any. */
  persona: AgentPersona | null;
  onDuplicatePersona: (persona: AgentPersona) => void;
  onEditPersona: (persona: AgentPersona) => void;
  onOpenAgentProfile: (pubkey: string) => void;
}) {
  const profile = useUserProfileQuery(agent.pubkey).data;
  const deviceLabels = useAuthDeviceLabels();
  const hostDevice = deviceLabels.get(deviceRobotTag(agent.deviceId) ?? "");
  const hostLabel = agentHostDeviceLabel(hostDevice);
  const title =
    persona?.displayName.trim() ||
    agent.name?.trim() ||
    profile?.displayName?.trim() ||
    "Agent";
  const avatarUrl =
    profile?.avatarUrl?.trim() || persona?.avatarUrl?.trim() || null;
  const availability = getAvailability(agent.pubkey);
  const stop = useRemoteAgentCommand(stopRemoteOwnedAgent, "stop");
  const start = useRemoteAgentCommand(startRemoteOwnedAgent, "start");

  return (
    <AgentIdentityCard
      actions={
        <RemoteAgentActionsMenu
          canStart={availability === "offline"}
          canStop={availability !== "offline"}
          isPending={isPending || stop.isPending || start.isPending}
          label={title}
          persona={persona}
          onDuplicatePersona={onDuplicatePersona}
          onEditPersona={onEditPersona}
          onStart={() =>
            start.run({ pubkey: agent.pubkey, hostName: hostDevice?.name })
          }
          onStop={() => stop.run({ pubkey: agent.pubkey })}
        />
      }
      ariaLabel={`${title} agent profile`}
      avatar={
        <AgentRuntimeAvatarControl
          activeTestId={`remote-agent-status-${agent.pubkey}`}
          availability={availability}
          avatarUrl={avatarUrl}
          hostedElsewhereLabel={hostLabel}
          isActive={availability === "online" || availability === "away"}
          isStarting={false}
          label={title}
          startTestId={`remote-agent-start-${agent.pubkey}`}
          onStart={() => {}}
        />
      }
      avatarUrl={avatarUrl}
      dataTestId={`remote-agent-row-${agent.pubkey}`}
      label={title}
      subtitle={hostLabel}
      onClick={() => onOpenAgentProfile(agent.pubkey)}
    />
  );
}

function RemoteAgentActionsMenu({
  canStart,
  canStop,
  isPending,
  label,
  persona,
  onDuplicatePersona,
  onEditPersona,
  onStart,
  onStop,
}: {
  canStart: boolean;
  canStop: boolean;
  isPending: boolean;
  label: string;
  persona: AgentPersona | null;
  onDuplicatePersona: (persona: AgentPersona) => void;
  onEditPersona: (persona: AgentPersona) => void;
  onStart: () => void;
  onStop: () => void;
}) {
  const canEdit = persona !== null && !persona.sourceTeam;
  return (
    <DropdownMenu modal={false}>
      <DropdownMenuTrigger asChild>
        <button
          aria-label={`Open actions for ${label}`}
          className="flex h-7 w-7 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
          type="button"
        >
          <EllipsisVertical className="h-4 w-4" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        align="end"
        onCloseAutoFocus={(event) => event.preventDefault()}
      >
        <DropdownMenuItem disabled={isPending || !canStart} onClick={onStart}>
          <Play className="h-4 w-4" />
          Start
        </DropdownMenuItem>
        <DropdownMenuItem disabled={isPending || !canStop} onClick={onStop}>
          <Square className="h-4 w-4" />
          Stop
        </DropdownMenuItem>
        {persona ? (
          <>
            <DropdownMenuSeparator />
            {canEdit ? (
              <DropdownMenuItem
                disabled={isPending}
                onClick={() => onEditPersona(persona)}
              >
                <Pencil className="h-4 w-4" />
                Edit
              </DropdownMenuItem>
            ) : null}
            <DropdownMenuItem
              disabled={isPending}
              onClick={() => onDuplicatePersona(persona)}
            >
              <CopyPlus className="h-4 w-4" />
              Duplicate
            </DropdownMenuItem>
          </>
        ) : null}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

type RemoteAgentCommandInput = { pubkey: string; hostName?: string | null };

function useRemoteAgentCommand(
  command: (
    input: RemoteAgentCommandInput & {
      channels: Channel[];
      relayAgents: RelayAgent[];
    },
  ) => Promise<ManagedAgentActionResult>,
  verb: "start" | "stop",
) {
  const channelsQuery = useChannelsQuery();
  const relayAgentsQuery = useRelayAgentsQuery();
  const [isPending, setIsPending] = React.useState(false);
  const run = React.useCallback(
    async (input: RemoteAgentCommandInput) => {
      setIsPending(true);
      try {
        const result = await command({
          ...input,
          channels: channelsQuery.data ?? [],
          relayAgents: relayAgentsQuery.data ?? [],
        });
        if (result.noticeMessage) toast.success(result.noticeMessage);
      } catch (error) {
        toast.error(
          error instanceof Error
            ? error.message
            : `Could not ${verb} the agent.`,
        );
      } finally {
        setIsPending(false);
      }
    },
    [channelsQuery.data, command, relayAgentsQuery.data, verb],
  );
  return { isPending, run };
}
