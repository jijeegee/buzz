import { AgentBadgeIcon } from "@/features/profile/ui/AgentBadgeIcon";
import { useIdentityQuery } from "@/shared/api/hooks";

/** The robot of the device one of my agents runs on, before its name. */
export function AgentCardRobot({ pubkey }: { pubkey: string }) {
  const viewer = useIdentityQuery().data?.pubkey ?? null;
  return (
    <AgentBadgeIcon
      agentPubkey={pubkey}
      className="h-4 w-4"
      fallback="none"
      othersSee="bot"
      ownerPubkey={viewer}
      testId={`agent-card-robot-${pubkey.toLowerCase()}`}
    />
  );
}
