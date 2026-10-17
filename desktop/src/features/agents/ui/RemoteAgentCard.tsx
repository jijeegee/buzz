import * as React from "react";
import { CopyPlus, EllipsisVertical, Pencil, Square } from "lucide-react";
import { toast } from "sonner";

import { useRelayAgentsQuery } from "@/features/agents/hooks";
import { stopRemoteOwnedAgent } from "@/features/agents/lib/managedAgentControlActions";
import type { AgentAvailabilityReader } from "@/features/agents/lib/useAgentAvailability";
import type { RemoteOwnedAgentHere } from "@/features/agents/useRemoteOwnedAgents";
import { useChannelsQuery } from "@/features/channels/hooks";
import { useUserProfileQuery } from "@/features/profile/hooks";
import { agentHostDeviceLabel } from "@/features/profile/ui/AgentBadgeIcon";
import type { AgentPersona } from "@/shared/api/types";
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
 * name and availability and offers no Start (it has no key for it). Stop asks
 * the agent itself to exit; edits to its persona reach the hosting computer
 * through persona sync.
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
  const hostLabel = agentHostDeviceLabel(
    deviceLabels.get(deviceRobotTag(agent.deviceId) ?? ""),
  );
  const title =
    persona?.displayName ??
    agent.name ??
    profile?.displayName?.trim() ??
    "Agent";
  const avatarUrl =
    profile?.avatarUrl?.trim() || persona?.avatarUrl?.trim() || null;
  const availability = getAvailability(agent.pubkey);
  const stop = useStopRemoteAgent();

  return (
    <AgentIdentityCard
      actions={
        <RemoteAgentActionsMenu
          canStop={availability !== "offline"}
          isPending={isPending || stop.isPending}
          label={title}
          persona={persona}
          onDuplicatePersona={onDuplicatePersona}
          onEditPersona={onEditPersona}
          onStop={() => stop.run(agent.pubkey)}
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
  canStop,
  isPending,
  label,
  persona,
  onDuplicatePersona,
  onEditPersona,
  onStop,
}: {
  canStop: boolean;
  isPending: boolean;
  label: string;
  persona: AgentPersona | null;
  onDuplicatePersona: (persona: AgentPersona) => void;
  onEditPersona: (persona: AgentPersona) => void;
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

function useStopRemoteAgent() {
  const channelsQuery = useChannelsQuery();
  const relayAgentsQuery = useRelayAgentsQuery();
  const [isPending, setIsPending] = React.useState(false);
  const run = React.useCallback(
    async (pubkey: string) => {
      setIsPending(true);
      try {
        const result = await stopRemoteOwnedAgent({
          pubkey,
          channels: channelsQuery.data ?? [],
          relayAgents: relayAgentsQuery.data ?? [],
        });
        if (result.noticeMessage) toast.success(result.noticeMessage);
      } catch (error) {
        toast.error(
          error instanceof Error ? error.message : "Could not stop the agent.",
        );
      } finally {
        setIsPending(false);
      }
    },
    [channelsQuery.data, relayAgentsQuery.data],
  );
  return { isPending, run };
}
