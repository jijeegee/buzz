import { skipToken, useQuery } from "@tanstack/react-query";
import { Bot } from "lucide-react";

import {
  type UsersBatchEntry,
  usersBatchEntryKey,
  useUserProfileQuery,
} from "@/features/profile/hooks";
import { useAgentBadge } from "@/features/profile/lib/useAgentBadge";
import { ProfileAvatar } from "@/features/profile/ui/ProfileAvatar";
import {
  type AuthDeviceLabel,
  useAuthDeviceLabels,
} from "@/shared/api/useAuthDeviceNames";
import { cn } from "@/shared/lib/cn";
import { truncateNpub } from "@/shared/lib/pubkey";
import { DeviceRobotIcon } from "@/shared/ui/DeviceRobotIcon";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/shared/ui/tooltip";

/**
 * Another account's agent: the owner's avatar, shrunk into the robot slot
 * with a robot corner badge, so it reads as "belongs to this person" without
 * passing for the person themselves.
 */
function OwnerMark({
  ownerPubkey,
  className,
  testId,
}: {
  ownerPubkey: string;
  className?: string;
  testId?: string;
}) {
  // Reuse the batch entry a surrounding list resolved; fetch only when absent.
  const entry = useQuery<UsersBatchEntry>({
    queryKey: usersBatchEntryKey(ownerPubkey),
    queryFn: skipToken,
  });
  const cached = entry.data?.summary;
  const profile = useUserProfileQuery(cached ? undefined : ownerPubkey);
  const avatarUrl = cached?.avatarUrl ?? profile.data?.avatarUrl ?? null;
  const name =
    cached?.displayName ??
    profile.data?.displayName ??
    truncateNpub(ownerPubkey);
  return (
    <span
      className={cn("relative inline-flex shrink-0", className)}
      data-agent-badge="owner"
      data-owner-pubkey={ownerPubkey}
      data-testid={testId ?? "agent-owner-mark"}
      title={`Agent owned by ${name}`}
    >
      <ProfileAvatar
        avatarUrl={avatarUrl}
        className="h-full w-full text-3xs shadow-none ring-1 ring-border"
        iconClassName="h-[70%] w-[70%]"
        label={name}
      />
      <span className="absolute -bottom-[18%] -right-[18%] flex h-[62%] w-[62%] items-center justify-center rounded-full bg-background text-muted-foreground">
        <Bot aria-hidden="true" className="h-[85%] w-[85%]" strokeWidth={2.5} />
      </span>
    </span>
  );
}

/** Tooltip copy for the device an agent runs on. */
export function agentHostDeviceLabel(device: AuthDeviceLabel | undefined) {
  const name = device?.name.trim();
  if (device?.current) {
    return name ? `Running on this device (${name})` : "Running on this device";
  }
  return name ? `Running on ${name}` : "Running on another device";
}

/**
 * The agent's robot slot as the viewer sees it: for the agent's owner, the
 * robot of the device it runs on, with the device's name on hover; for
 * everyone else an owner mark (or, with `othersSee="bot"`, the plain robot
 * a row showed before device robots existed); otherwise the default robot.
 * `fallback="none"` renders nothing instead of the default.
 */
export function AgentBadgeIcon({
  agentPubkey,
  ownerPubkey,
  className,
  defaultLabel,
  fallback = "bot",
  othersSee = "owner",
  testId,
}: {
  agentPubkey: string | null | undefined;
  /** The agent's owner as the surrounding row knows it; see `useAgentBadge`. */
  ownerPubkey?: string | null;
  className?: string;
  /** Accessible name for the default robot; decorative when omitted. */
  defaultLabel?: string;
  fallback?: "bot" | "none";
  othersSee?: "owner" | "bot";
  testId?: string;
}) {
  const badge = useAgentBadge(agentPubkey, ownerPubkey);
  const deviceLabels = useAuthDeviceLabels(badge.kind === "device");
  if (badge.kind === "owner" && othersSee === "owner") {
    return (
      <OwnerMark
        className={className}
        ownerPubkey={badge.ownerPubkey}
        testId={testId}
      />
    );
  }
  if (badge.kind === "device") {
    const label = agentHostDeviceLabel(deviceLabels.get(badge.variant.tag));
    return (
      <Tooltip>
        <TooltipTrigger asChild>
          <span
            aria-label={label}
            className={cn("inline-flex shrink-0", className)}
            data-agent-badge="device"
            data-device-id={badge.deviceId}
            data-testid={testId}
            role="img"
          >
            <DeviceRobotIcon
              className="h-full w-full"
              variant={badge.variant}
            />
          </span>
        </TooltipTrigger>
        <TooltipContent data-testid="agent-host-device-tooltip">
          {label}
        </TooltipContent>
      </Tooltip>
    );
  }
  if (fallback === "none") return null;
  return defaultLabel ? (
    <Bot
      aria-label={defaultLabel}
      className={className}
      data-agent-badge="default"
      data-testid={testId}
      role="img"
    />
  ) : (
    <Bot
      aria-hidden="true"
      className={className}
      data-agent-badge="default"
      data-testid={testId}
    />
  );
}
