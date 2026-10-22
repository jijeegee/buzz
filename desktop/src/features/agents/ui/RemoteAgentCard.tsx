import * as React from "react";
import { toast } from "sonner";

import { useAgentWorking } from "@/features/agents/agentWorkingSignal";
import {
  type ManagedAgentActionResult,
  startRemoteOwnedAgent,
  stopRemoteOwnedAgent,
} from "@/features/agents/lib/managedAgentControlActions";
import type { AgentAvailabilityReader } from "@/features/agents/lib/useAgentAvailability";
import type { RemoteOwnedAgentHere } from "@/features/agents/useRemoteOwnedAgents";
import { useUserProfileQuery } from "@/features/profile/hooks";
import type { AgentPersona } from "@/shared/api/types";
import { Button } from "@/shared/ui/button";
import { AgentCardRobot } from "./AgentCardRobot";
import { AgentTile } from "./AgentTile";

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
  hostName,
  isPending,
  persona,
  onEditPersona,
  onOpenAgentProfile,
}: {
  agent: RemoteOwnedAgentHere;
  getAvailability: AgentAvailabilityReader;
  /** The computer it runs on, as its device heading names it. */
  hostName: string;
  isPending: boolean;
  /** This computer's persona the agent was made from, if any. */
  persona: AgentPersona | null;
  onEditPersona: (persona: AgentPersona) => void;
  onOpenAgentProfile: (pubkey: string) => void;
}) {
  const profile = useUserProfileQuery(agent.pubkey).data;
  const title =
    agent.name?.trim() ||
    profile?.displayName?.trim() ||
    persona?.displayName.trim() ||
    "Agent";
  const avatarUrl =
    profile?.avatarUrl?.trim() || persona?.avatarUrl?.trim() || null;
  const availability = getAvailability(agent.pubkey);
  const working = useAgentWorking(agent.pubkey).working;
  const stop = useRemoteAgentCommand(stopRemoteOwnedAgent, "stop");
  const start = useRemoteAgentCommand(startRemoteOwnedAgent, "start");
  const isOnline = availability === "online" || availability === "away";
  const busy = isPending || stop.isPending || start.isPending;
  const canEdit = persona !== null && !persona.sourceTeam;

  return (
    <AgentTile
      avatarUrl={avatarUrl}
      buttons={
        <>
          {isOnline ? (
            <Button
              data-testid={`remote-agent-stop-${agent.pubkey}`}
              disabled={busy}
              onClick={() => stop.run({ pubkey: agent.pubkey })}
              size="xs"
              variant="outline"
            >
              Stop
            </Button>
          ) : (
            <Button
              data-testid={`remote-agent-start-${agent.pubkey}`}
              disabled={busy || availability === undefined}
              onClick={() => start.run({ pubkey: agent.pubkey, hostName })}
              size="xs"
            >
              Start
            </Button>
          )}
          <Button
            onClick={() =>
              canEdit
                ? onEditPersona(persona)
                : onOpenAgentProfile(agent.pubkey)
            }
            size="xs"
            variant="outline"
          >
            Edit
          </Button>
        </>
      }
      dataTestId={`remote-agent-row-${agent.pubkey}`}
      description={persona ? `from preset ${persona.displayName}` : null}
      label={title}
      labelBadge={<AgentCardRobot pubkey={agent.pubkey} />}
      openLabel={`${title} agent profile`}
      status={
        availability === undefined
          ? undefined
          : isOnline
            ? working
              ? "working"
              : "running"
            : "stopped"
      }
      subtitle={hostName}
      onOpen={() => onOpenAgentProfile(agent.pubkey)}
    />
  );
}

type RemoteAgentCommandInput = { pubkey: string; hostName?: string | null };

function useRemoteAgentCommand(
  command: (
    input: RemoteAgentCommandInput,
  ) => Promise<ManagedAgentActionResult>,
  verb: "start" | "stop",
) {
  const [isPending, setIsPending] = React.useState(false);
  const run = React.useCallback(
    async (input: RemoteAgentCommandInput) => {
      setIsPending(true);
      try {
        const result = await command(input);
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
    [command, verb],
  );
  return { isPending, run };
}
