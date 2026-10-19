import * as React from "react";
import { useLocation, useNavigate } from "@tanstack/react-router";
import {
  Hash,
  LayoutGrid,
  MessageSquare,
  MoreHorizontal,
  Pencil,
  Play,
  Plus,
  RotateCw,
  Square,
  UserRound,
} from "lucide-react";
import { toast } from "sonner";

import { useAppNavigation } from "@/app/navigation/useAppNavigation";

import { agentsOfAccount } from "@/features/accounts/lib/accountLabels";
import {
  useManagedAgentsQuery,
  usePersonasQuery,
} from "@/features/agents/hooks";
import {
  isManagedAgentActive,
  startRemoteOwnedAgent,
  stopRemoteOwnedAgent,
} from "@/features/agents/lib/managedAgentControlActions";
import {
  groupRemoteAgentsByDevice,
  localAgentsForList,
} from "@/features/agents/lib/myAgentDevices";
import { useAgentAvailabilityLookup } from "@/features/agents/lib/useAgentAvailability";
import { useAgentWorking } from "@/features/agents/agentWorkingSignal";
import { useRemoteOwnedAgents } from "@/features/agents/useRemoteOwnedAgents";
import { useChannelsQuery, useOpenDmMutation } from "@/features/channels/hooks";
import { useIsArchivedPredicate } from "@/features/identity-archive/hooks";
import { useUserProfileQuery } from "@/features/profile/hooks";
import { AgentBadgeIcon } from "@/features/profile/ui/AgentBadgeIcon";
import { ProfileAvatar } from "@/features/profile/ui/ProfileAvatar";
import { useIdentityQuery } from "@/shared/api/hooks";
import type { AgentPersona, Channel, ManagedAgent } from "@/shared/api/types";
import {
  useAuthDeviceLabels,
  useAuthDevicesQuery,
} from "@/shared/api/useAuthDeviceNames";
import { deviceRobotTag } from "@/shared/lib/deviceRobot";
import { useHistorySearchState } from "@/shared/hooks/useHistorySearchState";
import { cn } from "@/shared/lib/cn";
import { Button } from "@/shared/ui/button";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "@/shared/ui/context-menu";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/shared/ui/dropdown-menu";
import { Input } from "@/shared/ui/input";
import { Skeleton } from "@/shared/ui/skeleton";
import { deferMenuAction } from "@/features/sidebar/ui/sidebarMenuHelpers";
import { AddAgentToChannelDialog } from "./AddAgentToChannelDialog";
import { DeviceRobot } from "./MyAgentsSection";
import { AddMyTeamToChannelDialog } from "./MyTeamDialogs";
import { useManagedAgentActions } from "./useManagedAgentActions";

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
  /** This computer's record; `null` for an agent on another computer. */
  managed: ManagedAgent | null;
  /** Another computer's name, for asking it to start the agent. */
  hostName: string | null;
  /** Its status is not known yet (another computer, presence loading). */
  statusUnknown?: boolean;
};

/** What a row's menu does; the row's own click is `message`. */
type AgentRowAction =
  | "message"
  | "start"
  | "stop"
  | "restart"
  | "addToChannel"
  | "edit"
  | "profile";

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
    hostName: null,
    key: `agent:${agent.pubkey}`,
    managed: agent,
    name: agent.name,
    pubkey: agent.pubkey.toLowerCase(),
    running: isManagedAgentActive(agent),
  };
}

/** The other person in a DM with just the viewer, lowercase. */
function dmPeer(channel: Channel | undefined, viewer: string | null) {
  if (channel?.channelType !== "dm") return null;
  const others = channel.participantPubkeys
    .map((pubkey) => pubkey.toLowerCase())
    .filter((pubkey) => pubkey !== viewer?.toLowerCase());
  return others.length === 1 ? others[0] : null;
}

/**
 * The Agents tab's list, in the left column: only real agents, the same ones
 * the Agents page's "My agents" shows, this computer's first and then one
 * group per other computer, each under that computer's robot. Presets and
 * teams live on the Agents page. Picking an agent opens the DM with it on
 * the right; the row's menu (its "⋯" or a right click) starts, stops, edits
 * and adds it to a channel.
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
              hostName: group.name || null,
              key: `remote:${agent.pubkey}`,
              managed: null,
              name: agent.name?.trim() || "",
              pubkey: agent.pubkey.toLowerCase(),
              running: availability === "online" || availability === "away",
              statusUnknown: availability === undefined,
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

  const actions = useManagedAgentActions();
  const { setActionErrorMessage, setActionNoticeMessage } = actions;
  // The page shows these in its banner; here they are toasts.
  React.useEffect(() => {
    if (!actions.actionErrorMessage) return;
    toast.error(actions.actionErrorMessage);
    setActionErrorMessage(null);
  }, [actions.actionErrorMessage, setActionErrorMessage]);
  React.useEffect(() => {
    if (!actions.actionNoticeMessage) return;
    toast.success(actions.actionNoticeMessage);
    setActionNoticeMessage(null);
  }, [actions.actionNoticeMessage, setActionNoticeMessage]);
  const [remotePending, setRemotePending] = React.useState<string | null>(null);
  const [remoteToAdd, setRemoteToAdd] = React.useState<AgentListItem | null>(
    null,
  );
  const openDmMutation = useOpenDmMutation();
  const { goChannel } = useAppNavigation();
  const channelsQuery = useChannelsQuery();
  const openChannelId = pathname.startsWith("/channels/")
    ? decodeURIComponent(pathname.split("/")[2] ?? "")
    : null;
  const openDmPeer = React.useMemo(
    () =>
      dmPeer(
        openChannelId
          ? channelsQuery.data?.find((channel) => channel.id === openChannelId)
          : undefined,
        viewerPubkey,
      ),
    [channelsQuery.data, openChannelId, viewerPubkey],
  );

  const needle = filter.trim().toLowerCase();
  const isSelected = (item: AgentListItem) =>
    openDmPeer === item.pubkey ||
    (onAgentsPage && values.profile?.toLowerCase() === item.pubkey);
  const openDm = async (item: AgentListItem) => {
    try {
      const dm = await openDmMutation.mutateAsync({ pubkeys: [item.pubkey] });
      await goChannel(dm.id);
    } catch (error) {
      toast.error(
        error instanceof Error ? error.message : "Could not open the DM.",
      );
    }
  };
  const runRemote = async (item: AgentListItem, verb: "start" | "stop") => {
    setRemotePending(item.pubkey);
    try {
      const result =
        verb === "start"
          ? await startRemoteOwnedAgent({
              pubkey: item.pubkey,
              hostName: item.hostName,
            })
          : await stopRemoteOwnedAgent({ pubkey: item.pubkey });
      if (result.noticeMessage) toast.success(result.noticeMessage);
    } catch (error) {
      toast.error(
        error instanceof Error ? error.message : `Could not ${verb} the agent.`,
      );
    } finally {
      setRemotePending(null);
    }
  };
  const onAction = (item: AgentListItem, action: AgentRowAction) => {
    const managed = item.managed;
    switch (action) {
      case "message":
        void openDm(item);
        return;
      case "start":
        if (!managed) void runRemote(item, "start");
        else if (managed.needsRestart)
          void actions.handleRestart(managed.pubkey);
        else void actions.handleStart(managed.pubkey);
        return;
      case "stop":
        if (managed) void actions.handleStop(managed.pubkey);
        else void runRemote(item, "stop");
        return;
      case "restart":
        if (managed) void actions.handleRestart(managed.pubkey);
        return;
      case "addToChannel":
        if (managed) actions.setAgentToAddToChannel(managed);
        else setRemoteToAdd(item);
        return;
      case "edit":
        // Like the page card's Edit: this agent's own settings.
        void navigate({
          to: "/agents",
          search: managed
            ? { profile: item.pubkey, profileTab: "runtime" }
            : { profile: item.pubkey },
        });
        return;
      case "profile":
        void navigate({ to: "/agents", search: { profile: item.pubkey } });
        return;
    }
  };
  const isBusy = (item: AgentListItem) =>
    item.managed
      ? actions.isPending
      : remotePending === item.pubkey || item.statusUnknown === true;
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
                        isBusy={isBusy(item)}
                        isSelected={isSelected(item)}
                        item={item}
                        key={item.key}
                        onAction={onAction}
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
      {actions.agentToAddToChannel ? (
        <AddAgentToChannelDialog
          agent={actions.agentToAddToChannel}
          onAdded={actions.handleAddedToChannel}
          onOpenChange={(open) => {
            if (!open) actions.setAgentToAddToChannel(null);
          }}
          open
        />
      ) : null}
      <AddMyTeamToChannelDialog
        agentsByPubkey={
          new Map(
            remoteToAdd
              ? [
                  [
                    remoteToAdd.pubkey,
                    {
                      computer: remoteToAdd.hostName ?? "Another device",
                      name: remoteToAdd.name || "Agent",
                      pubkey: remoteToAdd.pubkey,
                      running: remoteToAdd.running,
                    },
                  ],
                ]
              : [],
          )
        }
        onAdded={(message) => toast.success(message)}
        onOpenChange={(open) => {
          if (!open) setRemoteToAdd(null);
        }}
        team={
          remoteToAdd
            ? {
                agentPubkeys: [remoteToAdd.pubkey],
                createdAt: 0,
                description: null,
                id: `agent:${remoteToAdd.pubkey}`,
                name: remoteToAdd.name || "Agent",
              }
            : null
        }
      />
    </div>
  );
}

/** One menu, drawn by the "⋯" dropdown and by the right-click menu. */
function AgentRowMenuItems({
  Item,
  Separator,
  isBusy,
  item,
  onAction,
}: {
  Item: typeof DropdownMenuItem | typeof ContextMenuItem;
  Separator: typeof DropdownMenuSeparator | typeof ContextMenuSeparator;
  isBusy: boolean;
  item: AgentListItem;
  onAction: (item: AgentListItem, action: AgentRowAction) => void;
}) {
  const run = (action: AgentRowAction) => () =>
    deferMenuAction(() => onAction(item, action));
  const testId = (action: string) => `agents-row-menu-${action}-${item.key}`;
  const iconClass = "h-4 w-4";
  return (
    <>
      <Item data-testid={testId("message")} onSelect={run("message")}>
        <MessageSquare className={iconClass} />
        Message
      </Item>
      {item.running ? (
        <Item
          data-testid={testId("stop")}
          disabled={isBusy}
          onSelect={run("stop")}
        >
          <Square className={iconClass} />
          Stop
        </Item>
      ) : (
        <Item
          data-testid={testId("start")}
          disabled={isBusy}
          onSelect={run("start")}
        >
          <Play className={iconClass} />
          Start
        </Item>
      )}
      {item.running && item.managed?.needsRestart ? (
        <Item
          data-testid={testId("restart")}
          disabled={isBusy}
          onSelect={run("restart")}
        >
          <RotateCw className={iconClass} />
          Restart
        </Item>
      ) : null}
      <Separator />
      <Item
        data-testid={testId("add-to-channel")}
        onSelect={run("addToChannel")}
      >
        <Hash className={iconClass} />
        Add to channel
      </Item>
      <Item data-testid={testId("edit")} onSelect={run("edit")}>
        <Pencil className={iconClass} />
        Edit
      </Item>
      <Item data-testid={testId("profile")} onSelect={run("profile")}>
        <UserRound className={iconClass} />
        View profile
      </Item>
    </>
  );
}

function AgentRow({
  isBusy,
  isSelected,
  item,
  onAction,
  viewerPubkey,
}: {
  isBusy: boolean;
  isSelected: boolean;
  item: AgentListItem;
  onAction: (item: AgentListItem, action: AgentRowAction) => void;
  viewerPubkey: string | null;
}) {
  const [menuOpen, setMenuOpen] = React.useState(false);
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
    <ContextMenu>
      <ContextMenuTrigger asChild>
        <div className="group relative">
          <button
            aria-current={isSelected || undefined}
            className={cn(
              "flex h-11 w-full items-center gap-2.5 rounded-md px-2 text-left text-sm text-sidebar-foreground outline-hidden transition-colors hover:bg-sidebar-accent focus-visible:ring-2 focus-visible:ring-sidebar-ring",
              isSelected && "bg-sidebar-accent",
            )}
            data-testid={`agents-row-${item.key}`}
            onClick={() => onAction(item, "message")}
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
              className={cn(
                "flex shrink-0 items-center gap-1.5 text-2xs text-sidebar-foreground/60",
                // The "⋯" button takes this place while the row is hovered.
                "group-focus-within:invisible group-hover:invisible",
                menuOpen && "invisible",
              )}
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
          <DropdownMenu onOpenChange={setMenuOpen}>
            <DropdownMenuTrigger asChild>
              <Button
                aria-label={`${name} actions`}
                className={cn(
                  "absolute right-1 top-1/2 h-7 w-7 -translate-y-1/2 text-sidebar-foreground/70 opacity-0 hover:text-sidebar-foreground focus-visible:opacity-100 group-hover:opacity-100",
                  menuOpen && "opacity-100",
                )}
                data-testid={`agents-row-menu-${item.key}`}
                size="icon"
                variant="ghost"
              >
                <MoreHorizontal className="h-4 w-4" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="start" side="right">
              <AgentRowMenuItems
                Item={DropdownMenuItem}
                Separator={DropdownMenuSeparator}
                isBusy={isBusy}
                item={item}
                onAction={onAction}
              />
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      </ContextMenuTrigger>
      <ContextMenuContent>
        <AgentRowMenuItems
          Item={ContextMenuItem}
          Separator={ContextMenuSeparator}
          isBusy={isBusy}
          item={item}
          onAction={onAction}
        />
      </ContextMenuContent>
    </ContextMenu>
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
