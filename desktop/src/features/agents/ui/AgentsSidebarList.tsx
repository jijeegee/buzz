import * as React from "react";
import { useLocation, useNavigate } from "@tanstack/react-router";
import { LayoutGrid, Plus, Users } from "lucide-react";

import {
  useManagedAgentsQuery,
  usePersonasQuery,
} from "@/features/agents/hooks";
import { isManagedAgentActive } from "@/features/agents/lib/managedAgentControlActions";
import { pickProfileAgent } from "@/features/agents/lib/pickProfileAgent";
import { useAgentAvailabilityLookup } from "@/features/agents/lib/useAgentAvailability";
import { useTeamsQuery } from "@/features/agents/teamHooks";
import { buildUnifiedGroups } from "@/features/agents/ui/unifiedAgentGroups";
import { useRemoteOwnedAgents } from "@/features/agents/useRemoteOwnedAgents";
import { useIsArchivedPredicate } from "@/features/identity-archive/hooks";
import { useUserProfileQuery } from "@/features/profile/hooks";
import { AgentBadgeIcon } from "@/features/profile/ui/AgentBadgeIcon";
import { ProfileAvatar } from "@/features/profile/ui/ProfileAvatar";
import { useIdentityQuery } from "@/shared/api/hooks";
import type { AgentPersona, AgentTeam, ManagedAgent } from "@/shared/api/types";
import { useAuthDeviceLabels } from "@/shared/api/useAuthDeviceNames";
import { deviceRobotTag } from "@/shared/lib/deviceRobot";
import { useHistorySearchState } from "@/shared/hooks/useHistorySearchState";
import { cn } from "@/shared/lib/cn";
import { Button } from "@/shared/ui/button";
import { Input } from "@/shared/ui/input";
import { Skeleton } from "@/shared/ui/skeleton";

/** One row: a persona (via its agent, if any) or a standalone agent. */
type AgentListItem = {
  key: string;
  name: string;
  avatarUrl: string | null;
  running: boolean;
  /** The agent's pubkey, for its device robot; `null` before it is started. */
  pubkey: string | null;
  /** Used when the agent's profile has no picture. */
  fallbackAvatarUrl?: string | null;
  /** Second line: the other computer it runs on. */
  hostLabel?: string;
  /** Opens the agent's profile; a persona without an agent opens the persona. */
  target: { profile: string } | { profilePersona: string };
};

const PROFILE_KEYS = ["profile", "profilePersona"] as const;

function agentItem(agent: ManagedAgent): AgentListItem {
  return {
    avatarUrl: agent.avatarUrl,
    key: `agent:${agent.pubkey}`,
    name: agent.name,
    pubkey: agent.pubkey.toLowerCase(),
    running: isManagedAgentActive(agent),
    target: { profile: agent.pubkey.toLowerCase() },
  };
}

/**
 * The Agents tab's list, in the left column: the same agents the Agents
 * page shows, split into this computer's agents and the ones running on the
 * account's other computers, each marked with its device robot, then the
 * agent teams. This tab sets agents up; picking an agent opens its profile
 * and settings on the right, picking a team shows it on the Agents page.
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
  const teamsQuery = useTeamsQuery();
  const isArchived = useIsArchivedPredicate();
  const viewerPubkey = useIdentityQuery().data?.pubkey ?? null;
  const { agents: remoteAgents, settling: remoteSettling } =
    useRemoteOwnedAgents();
  const { getAvailability: getRemoteAvailability } = useAgentAvailabilityLookup(
    React.useMemo(
      () => remoteAgents.map((agent) => agent.pubkey),
      [remoteAgents],
    ),
  );
  const deviceLabels = useAuthDeviceLabels(remoteAgents.length > 0);

  const sections = React.useMemo(() => {
    const { groups, ungrouped, unknown } = buildUnifiedGroups(
      personasQuery.data ?? [],
      managedQuery.data ?? [],
      isArchived,
    );
    const personaById = new Map<string, AgentPersona>(
      (personasQuery.data ?? []).map((persona) => [persona.id, persona]),
    );
    const elsewhere = remoteAgents.filter((agent) => !isArchived(agent.pubkey));
    // Same split as the Agents page: a persona whose only agent runs on
    // another computer is listed there, not here as a fresh Start.
    const hostedElsewhere = new Set(
      elsewhere
        .map((agent) => agent.localPersonaId)
        .filter((id): id is string => id !== null),
    );
    const personas = groups.flatMap((group): AgentListItem[] => {
      const agent = pickProfileAgent(group.agents, isArchived);
      if (!agent && hostedElsewhere.has(group.persona.id)) return [];
      return [
        {
          avatarUrl: agent?.avatarUrl ?? group.persona.avatarUrl,
          key: `persona:${group.persona.id}`,
          name: group.persona.displayName,
          pubkey: agent ? agent.pubkey.toLowerCase() : null,
          running: agent ? isManagedAgentActive(agent) : false,
          target: agent
            ? { profile: agent.pubkey.toLowerCase() }
            : { profilePersona: group.persona.id },
        },
      ];
    });
    const remote = elsewhere.map((agent): AgentListItem => {
      const persona = agent.localPersonaId
        ? personaById.get(agent.localPersonaId)
        : undefined;
      const device = deviceLabels.get(deviceRobotTag(agent.deviceId) ?? "");
      const availability = getRemoteAvailability(agent.pubkey);
      return {
        // Like the page card: the agent's own profile first.
        avatarUrl: null,
        fallbackAvatarUrl: persona?.avatarUrl ?? null,
        hostLabel: device?.name.trim() || "Another device",
        key: `remote:${agent.pubkey}`,
        name: persona?.displayName.trim() || agent.name?.trim() || "",
        pubkey: agent.pubkey,
        running: availability === "online" || availability === "away",
        target: { profile: agent.pubkey },
      };
    });
    return [
      {
        key: "personas",
        title: "This device",
        items: [
          ...personas,
          ...ungrouped.map(agentItem),
          ...unknown.map(agentItem),
        ],
      },
      { key: "remote", title: "Other devices", items: remote },
    ];
  }, [
    deviceLabels,
    getRemoteAvailability,
    isArchived,
    managedQuery.data,
    personasQuery.data,
    remoteAgents,
  ]);

  const needle = filter.trim().toLowerCase();
  const isSelected = (item: AgentListItem) =>
    onAgentsPage &&
    ("profile" in item.target
      ? values.profile?.toLowerCase() === item.target.profile
      : values.profilePersona === item.target.profilePersona);
  const onPick = (item: AgentListItem) =>
    void navigate({ to: "/agents", search: item.target });
  const onPickTeam = (team: AgentTeam) => {
    void navigate({ to: "/agents", search: {} }).then(() =>
      revealTeamCard(team.id),
    );
  };
  const teams = (teamsQuery.data ?? []).filter(
    (team) => needle.length === 0 || team.name.toLowerCase().includes(needle),
  );
  // Until it is known which agents run elsewhere, a persona could still
  // move between the groups (and offer a Start for an agent made elsewhere).
  const isLoading =
    managedQuery.isLoading || personasQuery.isLoading || remoteSettling;

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
            // My agents always shows (with its empty note); others only when filled.
            if (items.length === 0 && section.key !== "personas") return null;
            return (
              <section data-testid={`agents-${section.key}`} key={section.key}>
                <h3 className="px-2 pb-1 text-2xs font-semibold uppercase tracking-wide text-sidebar-foreground/55">
                  {section.title}
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
        {!isLoading && teams.length > 0 ? (
          <section data-testid="agents-teams">
            <h3 className="px-2 pb-1 text-2xs font-semibold uppercase tracking-wide text-sidebar-foreground/55">
              Agent teams
            </h3>
            <div className="flex flex-col">
              {teams.map((team) => (
                <TeamRow
                  key={team.id}
                  personas={personasQuery.data ?? []}
                  team={team}
                  onPick={onPickTeam}
                />
              ))}
            </div>
          </section>
        ) : null}
      </div>
    </div>
  );
}

/**
 * Scrolls the team's card on the Agents page into view once the page has
 * drawn it (the page loads lazily), with a brief ring so it stands out.
 */
function revealTeamCard(teamId: string) {
  const deadline = Date.now() + 3000;
  const attempt = () => {
    const card = document.querySelector<HTMLElement>(
      `[data-testid="team-card-${CSS.escape(teamId)}"]`,
    );
    if (card) {
      card.scrollIntoView({ behavior: "smooth", block: "center" });
      card.classList.add("ring-2", "ring-primary");
      window.setTimeout(
        () => card.classList.remove("ring-2", "ring-primary"),
        1600,
      );
      return;
    }
    if (Date.now() < deadline) window.requestAnimationFrame(attempt);
  };
  window.requestAnimationFrame(attempt);
}

function TeamRow({
  personas,
  team,
  onPick,
}: {
  personas: AgentPersona[];
  team: AgentTeam;
  onPick: (team: AgentTeam) => void;
}) {
  const members = team.personaIds
    .map((id) => personas.find((persona) => persona.id === id))
    .filter((persona): persona is AgentPersona => persona !== undefined);
  return (
    <button
      className="flex h-12 w-full items-center gap-2.5 rounded-md px-2 text-left text-sm text-sidebar-foreground outline-hidden transition-colors hover:bg-sidebar-accent focus-visible:ring-2 focus-visible:ring-sidebar-ring"
      data-testid={`agents-team-row-${team.id}`}
      onClick={() => onPick(team)}
      type="button"
    >
      <span className="flex h-9 w-9 shrink-0 items-center justify-center rounded-full bg-sidebar-accent text-sidebar-foreground/70">
        <Users aria-hidden="true" className="h-4 w-4" />
      </span>
      <span className="flex min-w-0 flex-1 flex-col">
        <span className="truncate font-medium">{team.name}</span>
        <span className="truncate text-xs text-sidebar-foreground/60">
          {members.length > 0
            ? members.map((persona) => persona.displayName).join(", ")
            : `${team.personaIds.length} agents`}
        </span>
      </span>
    </button>
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
    item.avatarUrl ? undefined : (item.pubkey ?? undefined),
  ).data;
  const avatarUrl =
    item.avatarUrl ?? profile?.avatarUrl ?? item.fallbackAvatarUrl ?? null;
  const name = item.name || profile?.displayName?.trim() || "Agent";
  return (
    <button
      aria-current={isSelected || undefined}
      className={cn(
        "flex h-12 w-full items-center gap-2.5 rounded-md px-2 text-left text-sm text-sidebar-foreground outline-hidden transition-colors hover:bg-sidebar-accent focus-visible:ring-2 focus-visible:ring-sidebar-ring",
        isSelected && "bg-sidebar-accent",
      )}
      data-testid={`agents-row-${item.key}`}
      onClick={() => onPick(item)}
      type="button"
    >
      <span className="relative shrink-0">
        <ProfileAvatar
          avatarUrl={avatarUrl}
          className="h-9 w-9 text-xs shadow-none"
          iconClassName="h-4 w-4"
          label={name}
          shape="circle"
        />
        <span
          aria-label={item.running ? "Running" : "Stopped"}
          className={cn(
            "absolute -bottom-0.5 -right-0.5 h-3 w-3 rounded-full ring-2 ring-background",
            item.running ? "bg-emerald-500" : "bg-muted-foreground/40",
          )}
          role="img"
        />
      </span>
      <span className="flex min-w-0 flex-1 flex-col">
        <span className="flex min-w-0 items-center gap-1">
          {item.pubkey ? (
            <AgentBadgeIcon
              agentPubkey={item.pubkey}
              className="h-4 w-4"
              fallback="none"
              othersSee="bot"
              ownerPubkey={viewerPubkey}
              testId={`agents-row-robot-${item.key}`}
            />
          ) : null}
          <span className="min-w-0 truncate font-medium">{name}</span>
        </span>
        {item.hostLabel ? (
          <span className="truncate text-xs text-sidebar-foreground/60">
            {item.hostLabel}
          </span>
        ) : null}
      </span>
    </button>
  );
}

function ListSkeleton() {
  return (
    <div className="space-y-3 px-2 py-2">
      {[1, 2, 3].map((i) => (
        <div className="flex items-center gap-2.5" key={i}>
          <Skeleton className="h-9 w-9 shrink-0 rounded-full" />
          <Skeleton className="h-4 w-28" />
        </div>
      ))}
    </div>
  );
}
