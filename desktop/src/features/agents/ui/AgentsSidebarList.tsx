import * as React from "react";
import { useLocation, useNavigate } from "@tanstack/react-router";
import { LayoutGrid, Plus } from "lucide-react";

import {
  useManagedAgentsQuery,
  usePersonasQuery,
} from "@/features/agents/hooks";
import { isManagedAgentActive } from "@/features/agents/lib/managedAgentControlActions";
import { pickProfileAgent } from "@/features/agents/lib/pickProfileAgent";
import { buildUnifiedGroups } from "@/features/agents/ui/unifiedAgentGroups";
import { useIsArchivedPredicate } from "@/features/identity-archive/hooks";
import { ProfileAvatar } from "@/features/profile/ui/ProfileAvatar";
import type { ManagedAgent } from "@/shared/api/types";
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
  /** Opens the agent's profile; a persona without an agent opens the persona. */
  target: { profile: string } | { profilePersona: string };
};

const PROFILE_KEYS = ["profile", "profilePersona"] as const;

function agentItem(agent: ManagedAgent): AgentListItem {
  return {
    avatarUrl: agent.avatarUrl,
    key: `agent:${agent.pubkey}`,
    name: agent.name,
    running: isManagedAgentActive(agent),
    target: { profile: agent.pubkey.toLowerCase() },
  };
}

/**
 * The Agents tab's list, in the left column: the same agents the Agents
 * page shows (my personas, then custom and unknown agents). This tab sets
 * agents up; picking one opens its profile and settings on the right.
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

  const sections = React.useMemo(() => {
    const { groups, ungrouped, unknown } = buildUnifiedGroups(
      personasQuery.data ?? [],
      managedQuery.data ?? [],
      isArchived,
    );
    const personas = groups.map((group): AgentListItem => {
      const agent = pickProfileAgent(group.agents, isArchived);
      return {
        avatarUrl: agent?.avatarUrl ?? group.persona.avatarUrl,
        key: `persona:${group.persona.id}`,
        name: group.persona.displayName,
        running: agent ? isManagedAgentActive(agent) : false,
        target: agent
          ? { profile: agent.pubkey.toLowerCase() }
          : { profilePersona: group.persona.id },
      };
    });
    return [
      { key: "personas", title: "My agents", items: personas },
      {
        key: "custom",
        title: "Custom agents",
        items: ungrouped.map(agentItem),
      },
      {
        key: "unknown",
        title: "Unknown agents",
        items: unknown.map(agentItem),
      },
    ];
  }, [isArchived, managedQuery.data, personasQuery.data]);

  const needle = filter.trim().toLowerCase();
  const isSelected = (item: AgentListItem) =>
    onAgentsPage &&
    ("profile" in item.target
      ? values.profile?.toLowerCase() === item.target.profile
      : values.profilePersona === item.target.profilePersona);
  const onPick = (item: AgentListItem) =>
    void navigate({ to: "/agents", search: item.target });
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
}: {
  isSelected: boolean;
  item: AgentListItem;
  onPick: (item: AgentListItem) => void;
}) {
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
          avatarUrl={item.avatarUrl}
          className="h-9 w-9 text-xs shadow-none"
          iconClassName="h-4 w-4"
          label={item.name}
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
      <span className="min-w-0 flex-1 truncate font-medium">{item.name}</span>
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
