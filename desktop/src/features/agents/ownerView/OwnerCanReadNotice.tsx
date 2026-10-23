import { Eye } from "lucide-react";
import * as React from "react";

import { isGroupChat } from "@/features/channels/lib/groupChat";
import { useUsersBatchQuery } from "@/features/profile/hooks";
import type {
  Channel,
  ChannelMember,
  UserProfileSummary,
} from "@/shared/api/types";
import { useT } from "@/shared/i18n";

export type OwnerReader = { agentPubkey: string; ownerPubkey: string };

/**
 * Owners who can read this chat without being in it: the owner of each
 * agent here, unless that owner is a member or is me. Only DMs and group
 * chats are readable by an agent's owner.
 */
export function ownerReaders(
  channel: Channel | null,
  members: readonly ChannelMember[] | undefined,
  profiles: Readonly<Record<string, UserProfileSummary>> | undefined,
  currentPubkey: string | undefined,
): OwnerReader[] {
  if (!channel || !members) return [];
  if (channel.channelType !== "dm" && !isGroupChat(channel)) return [];
  const memberSet = new Set(members.map((m) => m.pubkey.toLowerCase()));
  const me = currentPubkey?.toLowerCase();
  const seen = new Set<string>();
  const readers: OwnerReader[] = [];
  for (const member of members) {
    const agentPubkey = member.pubkey.toLowerCase();
    const profile = profiles?.[agentPubkey];
    if (!member.isAgent && !profile?.isAgent) continue;
    const ownerPubkey = profile?.ownerPubkey?.toLowerCase();
    if (!ownerPubkey || ownerPubkey === me || memberSet.has(ownerPubkey)) {
      continue;
    }
    const key = `${ownerPubkey}:${agentPubkey}`;
    if (seen.has(key)) continue;
    seen.add(key);
    readers.push({ agentPubkey, ownerPubkey });
  }
  return readers;
}

/** "akak (Fizz's owner) can read this chat": a header chip per agent. */
export function OwnerCanReadNotice({
  channel,
  currentPubkey,
  members,
}: {
  channel: Channel | null;
  currentPubkey: string | undefined;
  members: readonly ChannelMember[] | undefined;
}) {
  const t = useT();
  const applies =
    channel !== null &&
    (channel.channelType === "dm" || isGroupChat(channel)) &&
    (members?.length ?? 0) > 0;
  const agentPubkeys = React.useMemo(
    () =>
      applies
        ? (members ?? [])
            .filter((member) => member.isAgent)
            .map((member) => member.pubkey)
        : [],
    [applies, members],
  );
  const agentProfiles = useUsersBatchQuery(agentPubkeys, {
    enabled: agentPubkeys.length > 0,
  }).data?.profiles;
  const readers = React.useMemo(
    () => ownerReaders(channel, members, agentProfiles, currentPubkey),
    [agentProfiles, channel, currentPubkey, members],
  );
  const ownerProfiles = useUsersBatchQuery(
    readers.map((reader) => reader.ownerPubkey),
    { enabled: readers.length > 0 },
  ).data?.profiles;
  if (readers.length === 0) return null;
  return (
    <>
      {readers.map((reader) => (
        <span
          className="flex min-w-0 items-center gap-1 rounded-full bg-muted px-2 py-0.5 text-xs text-muted-foreground"
          data-testid="owner-can-read-notice"
          key={reader.agentPubkey}
        >
          <Eye aria-hidden="true" className="h-3 w-3 shrink-0" />
          <span className="truncate">
            {t("ownerView.notice", {
              agent:
                agentProfiles?.[reader.agentPubkey]?.displayName?.trim() ||
                t("ownerView.agentFallback"),
              owner:
                ownerProfiles?.[reader.ownerPubkey]?.displayName?.trim() ||
                t("ownerView.ownerFallback"),
            })}
          </span>
        </span>
      ))}
    </>
  );
}
