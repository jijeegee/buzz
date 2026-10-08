import { skipToken, useQuery } from "@tanstack/react-query";
import { Bot } from "lucide-react";

import {
  type UsersBatchEntry,
  usersBatchEntryKey,
  useUserProfileQuery,
} from "@/features/profile/hooks";
import { useAgentBadge } from "@/features/profile/lib/useAgentBadge";
import { ProfileAvatar } from "@/features/profile/ui/ProfileAvatar";
import { useAuthDeviceNames } from "@/shared/api/useAuthDeviceNames";
import { cn } from "@/shared/lib/cn";
import { truncateNpub } from "@/shared/lib/pubkey";
import { DeviceRobotIcon } from "@/shared/ui/DeviceRobotIcon";

/**
 * Another account's agent: the owner's avatar, shrunk into the robot slot
 * with a robot corner badge, so it reads as "belongs to this person" without
 * passing for the person themselves.
 */
function OwnerMark({
  ownerPubkey,
  className,
}: {
  ownerPubkey: string;
  className?: string;
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
      data-owner-pubkey={ownerPubkey}
      data-testid="agent-owner-mark"
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

/**
 * The agent's robot slot as the viewer sees it: the device robot (titled with
 * the device's name) for the owner, an owner mark for everyone else, or the
 * default robot. `fallback="none"` renders nothing instead of the default.
 */
export function AgentBadgeIcon({
  agentPubkey,
  className,
  fallback = "bot",
}: {
  agentPubkey: string | null | undefined;
  className?: string;
  fallback?: "bot" | "none";
}) {
  const badge = useAgentBadge(agentPubkey);
  const deviceNames = useAuthDeviceNames(badge.kind === "device");
  if (badge.kind === "owner") {
    return <OwnerMark className={className} ownerPubkey={badge.ownerPubkey} />;
  }
  if (badge.kind === "device") {
    const deviceName = deviceNames.get(badge.variant.tag);
    return (
      <span
        className={cn("inline-flex shrink-0", className)}
        title={deviceName ? `Running on ${deviceName}` : undefined}
      >
        <DeviceRobotIcon className="h-full w-full" variant={badge.variant} />
      </span>
    );
  }
  return fallback === "none" ? null : (
    <DeviceRobotIcon className={className} variant={null} />
  );
}
