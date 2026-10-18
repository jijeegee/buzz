import * as React from "react";
import { useLocation, useNavigate } from "@tanstack/react-router";
import { LayoutGrid, Plus } from "lucide-react";

import { agentsOfAccount } from "@/features/accounts/lib/accountLabels";
import {
  useManagedAgentsQuery,
  usePersonasQuery,
} from "@/features/agents/hooks";
import { isManagedAgentActive } from "@/features/agents/lib/managedAgentControlActions";
import {
  groupRemoteAgentsByDevice,
  localAgentsForList,
} from "@/features/agents/lib/myAgentDevices";
import { useAgentAvailabilityLookup } from "@/features/agents/lib/useAgentAvailability";
import { useAgentWorking } from "@/features/agents/agentWorkingSignal";
import { useRemoteOwnedAgents } from "@/features/agents/useRemoteOwnedAgents";
import { useIsArchivedPredicate } from "@/features/identity-archive/hooks";
import { useUserProfileQuery } from "@/features/profile/hooks";
import { AgentBadgeIcon } from "@/features/profile/ui/AgentBadgeIcon";
import { ProfileAvatar } from "@/features/profile/ui/ProfileAvatar";
import { useIdentityQuery } from "@/shared/api/hooks";
import type { AgentPersona, ManagedAgent } from "@/shared/api/types";
import {
  useAuthDeviceLabels,
  useAuthDevicesQuery,
} from "@/shared/api/useAuthDeviceNames";
import { deviceRobotTag } from "@/shared/lib/deviceRobot";
import { useHistorySearchState } from "@/shared/hooks/useHistorySearchState";
import { cn } from "@/shared/lib/cn";
import { Button } from "@/shared/ui/button";
import { Input } from "@/shared/ui/input";
import { Skeleton } from "@/shared/ui/skeleton";
import { DeviceRobot } from "./MyAgentsSection";

/** One row: one real agent, here or on another computer. */
type AgentListItem = {
  key: string;
  name: string;
  avatarUrl: string | null;
  running: boolean;
  /** The agent's pubkey (lowercase), for its device robot and profile. */
  pubkey: string;
  /** Used when the agent's profile has no picture. */
  fallbackAvatarUrl?: string | null;
};

type AgentListSection = {
  key: string;
  title: string;
  /** The computer, for its robot; `null` while this one is not known. */
  deviceId: string | null;
  items: AgentListItem[];
};

const PROFILE_KEYS = ["profile"] as const;

function agentItem(agent: ManagedAgent): AgentListItem {
  return {
    avatarUrl: agent.avatarUrl,
    key: `agent:${agent.pubkey}`,
    name: agent.name,
    pubkey: agent.pubkey.toLowerCase(),
    running: isManagedAgentActive(agent),
  };
}

/**
 * The Agents tab's list, in the left column: only real agents, the same ones
 * the Agents page's "My agents" shows, this computer's first and then one
 * group per other computer, each under that computer's robot. Presets and
 * teams live on the Agents page. Picking an agent opens its profile and
 * settings on the right.
 */
export function AgentsSidebarList({
  onCreateAgent,
}: {
  onCreateAgent: () => void;
}) {
  const navigate = useNavigate();
  const pathname = useLocation({ select: (location) => location.pathname });
  const { values } = useHistorySearchState(PROFILE_KEYS);
  const onAgentsPage = pathname === "/agents";
  const [filter, setFilter] = React.useState("");
  const managedQuery = useManagedAgentsQuery();
  const personasQuery = usePersonasQuery();
  const isArchived = useIsArchivedPredicate();
  const viewerPubkey = useIdentityQuery().data?.pubkey ?? null;
  const currentDeviceId =
    useAuthDevicesQuery().data?.find((device) => device.current)?.id ?? null;
  const { agents: remoteAgents } = useRemoteOwnedAgents();
  const { getAvailability: getRemoteAvailability } = useAgentAvailabilityLookup(
    React.useMemo(
      () => remoteAgents.map((agent) => agent.pubkey),
      [remoteAgents],
    ),
  );
  const deviceLabels = useAuthDeviceLabels(remoteAgents.length > 0);

  const sections = React.useMemo((): AgentListSection[] => {
    const personaById = new Map<string, AgentPersona>(
      (personasQuery.data ?? []).map((persona) => [persona.id, persona]),
    );
    const personaOf = (agent: { localPersonaId: string | null }) =>
      agent.localPersonaId ? personaById.get(agent.localPersonaId) : undefined;
    const remoteGroups = groupRemoteAgentsByDevice(
      remoteAgents.filter((agent) => !isArchived(agent.pubkey)),
      (deviceId) => deviceLabels.get(deviceRobotTag(deviceId) ?? "")?.name,
      (agent) => agent.name ?? personaOf(agent)?.displayName ?? "",
    );
    return [
      {
        key: "local",
        title: "This device",
        deviceId: currentDeviceId,
        // Like the page: only the active account's agents.
        items: localAgentsForList(
          agentsOfAccount(managedQuery.data ?? [], viewerPubkey),
          isArchived,
        ).map(agentItem),
      },
      ...remoteGroups.map(
        (group): AgentListSection => ({
          key: `device-${group.deviceId}`,
          title: group.name || "Another device",
          deviceId: group.deviceId,
          items: group.agents.map((agent): AgentListItem => {
            const availability = getRemoteAvailability(agent.pubkey);
            return {
              // Like the page card: the agent's own profile first.
              avatarUrl: null,
              fallbackAvatarUrl: personaOf(agent)?.avatarUrl ?? null,
              key: `remote:${agent.pubkey}`,
              name: agent.name?.trim() || "",
              pubkey: agent.pubkey.toLowerCase(),
              running: availability === "online" || availability === "away",
            };
          }),
        }),
      ),
    ];
  }, [
    currentDeviceId,
    deviceLabels,
    getRemoteAvailability,
    isArchived,
    managedQuery.data,
    personasQuery.data,
    remoteAgents,
    viewerPubkey,
  ]);

  const needle = filter.trim().toLowerCase();
  const isSelected = (item: AgentListItem) =>
    onAgentsPage && values.profile?.toLowerCase() === item.pubkey;
  const onPick = (item: AgentListItem) =>
    void navigate({ to: "/agents", search: { profile: item.pubkey } });
  const isLoading = managedQuery.isLoading || personasQuery.isLoading;

  return (
    <div className="flex min-h-0 flex-1 flex-col" data-testid="agents-list">
      <div className="space-y-2 px-2 pb-2 pt-1">
        <div className="flex items-center gap-1.5 px-1">
          <h2 className="min-w-0 flex-1 truncate text-base font-semibold text-sidebar-foreground">
            Agents
          </h2>
          <Button
            className="h-7 gap-1 px-2 text-xs"
            data-testid="agents-list-all"
            onClick={() => void navigate({ to: "/agents", search: {} })}
            size="sm"
            variant="outline"
          >
            <LayoutGrid className="h-3.5 w-3.5" />
            All agents
          </Button>
          <Button
            className="h-7 gap-1 px-2 text-xs"
            data-testid="agents-list-create"
            onClick={onCreateAgent}
            size="sm"
          >
            <Plus className="h-3.5 w-3.5" />
            New
          </Button>
        </div>
        <Input
          aria-label="Find agents by name"
          className="h-8 bg-background"
          data-testid="agents-filter"
          onChange={(event) => setFilter(event.target.value)}
          placeholder="Find agents"
          value={filter}
        />
      </div>
      <div className="min-h-0 flex-1 space-y-3 overflow-y-auto overflow-x-hidden px-1 pb-4">
        {isLoading ? (
          <ListSkeleton />
        ) : (
          sections.map((section) => {
            const items = section.items.filter(
              (item) =>
                needle.length === 0 || item.name.toLowerCase().includes(needle),
            );
            // This device always shows (with its empty note); others only when filled.
            if (items.length === 0 && section.key !== "local") return null;
            return (
              <section data-testid={`agents-${section.key}`} key={section.key}>
                <h3 className="flex min-w-0 items-center gap-1.5 px-2 pb-1 text-2xs font-semibold uppercase tracking-wide text-sidebar-foreground/55">
                  <span className="flex h-4 w-4 shrink-0 empty:hidden [&_svg]:h-4 [&_svg]:w-4">
                    <DeviceRobot deviceId={section.deviceId} />
                  </span>
                  <span className="truncate">{section.title}</span>
                </h3>
                {items.length > 0 ? (
                  <div className="flex flex-col">
                    {items.map((item) => (
                      <AgentRow
                        isSelected={isSelected(item)}
                        item={item}
                        key={item.key}
                        onPick={onPick}
                        viewerPubkey={viewerPubkey}
                      />
                    ))}
                  </div>
                ) : (
                  <p className="px-2 py-3 text-xs text-sidebar-foreground/60">
                    {needle ? "No agents match that name." : "No agents yet."}
                  </p>
                )}
              </section>
            );
          })
        )}
      </div>
    </div>
  );
}

function AgentRow({
  isSelected,
  item,
  onPick,
  viewerPubkey,
}: {
  isSelected: boolean;
  item: AgentListItem;
  onPick: (item: AgentListItem) => void;
  viewerPubkey: string | null;
}) {
  // A remote agent has no record here; its own profile has the picture.
  const profile = useUserProfileQuery(
    item.avatarUrl ? undefined : item.pubkey,
  ).data;
  const avatarUrl =
    item.avatarUrl ?? profile?.avatarUrl ?? item.fallbackAvatarUrl ?? null;
  const name = item.name || profile?.displayName?.trim() || "Agent";
  const working = useAgentWorking(item.running ? item.pubkey : null).working;
  const status = !item.running ? "Stopped" : working ? "Working" : "Running";
  return (
    <button
      aria-current={isSelected || undefined}
      className={cn(
        "flex h-11 w-full items-center gap-2.5 rounded-md px-2 text-left text-sm text-sidebar-foreground outline-hidden transition-colors hover:bg-sidebar-accent focus-visible:ring-2 focus-visible:ring-sidebar-ring",
        isSelected && "bg-sidebar-accent",
      )}
      data-testid={`agents-row-${item.key}`}
      onClick={() => onPick(item)}
      type="button"
    >
      <ProfileAvatar
        avatarUrl={avatarUrl}
        className="h-8 w-8 shrink-0 text-xs shadow-none"
        iconClassName="h-4 w-4"
        label={name}
        shape="circle"
      />
      <span className="flex min-w-0 flex-1 items-center gap-1">
        <AgentBadgeIcon
          agentPubkey={item.pubkey}
          className="h-4 w-4"
          fallback="none"
          othersSee="bot"
          ownerPubkey={viewerPubkey}
          testId={`agents-row-robot-${item.key}`}
        />
        <span className="min-w-0 truncate font-medium">{name}</span>
      </span>
      <span
        className="flex shrink-0 items-center gap-1.5 text-2xs text-sidebar-foreground/60"
        data-testid={`agents-row-status-${item.key}`}
      >
        <span
          aria-hidden
          className={cn(
            "h-2 w-2 rounded-full",
            status === "Working"
              ? "bg-amber-500"
              : status === "Running"
                ? "bg-emerald-500"
                : "bg-muted-foreground/40",
          )}
        />
        {status}
      </span>
    </button>
  );
}

function ListSkeleton() {
  return (
    <div className="space-y-3 px-2 py-2">
      {[1, 2, 3].map((i) => (
        <div className="flex items-center gap-2.5" key={i}>
          <Skeleton className="h-8 w-8 shrink-0 rounded-full" />
          <Skeleton className="h-4 w-28" />
        </div>
      ))}
    </div>
  );
}
