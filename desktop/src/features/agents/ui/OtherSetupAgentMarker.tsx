import { Cloud } from "lucide-react";

import { useIsOtherSetupAgent } from "../useKnownAgentPubkeys";

import { cn } from "@/shared/lib/cn";
import { useT } from "@/shared/i18n";

export function OtherSetupAgentMarker({
  className,
  testId,
}: {
  className?: string;
  testId?: string;
}) {
  const t = useT();
  return (
    <span
      aria-label={t("agents.otherSetup.label")}
      className={cn("inline-flex shrink-0", className)}
      data-testid={testId}
      role="img"
      title={t("agents.otherSetup.label")}
    >
      <Cloud aria-hidden="true" className="h-3 w-3" />
    </span>
  );
}

/** Connected marker for identity details; shares the app's directory subscriptions. */
export function AgentManagementMarker({
  pubkey,
  ownerPubkey,
  className,
  testId,
}: {
  pubkey?: string | null;
  ownerPubkey?: string | null;
  className?: string;
  testId?: string;
}) {
  const show = useIsOtherSetupAgent(pubkey, ownerPubkey);
  return show ? (
    <OtherSetupAgentMarker className={className} testId={testId} />
  ) : null;
}
