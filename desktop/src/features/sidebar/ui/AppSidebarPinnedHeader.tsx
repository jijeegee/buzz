import {
  Activity,
  Bot,
  Ellipsis,
  Folders,
  Inbox,
  MessagesSquare,
  Users,
  Zap,
} from "lucide-react";

import { TopbarSearch } from "@/features/search/ui/TopbarSearch";
import { SidebarProjectsSection } from "@/features/sidebar/ui/SidebarProjectsSection";
import { useFeatureEnabled } from "@/shared/features";
import type { Channel, SearchHit } from "@/shared/api/types";
import {
  SidebarHeader,
  SidebarMenu,
  SidebarMenuBadge,
  SidebarMenuButton,
  SidebarMenuItem,
} from "@/shared/ui/sidebar";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/shared/ui/dropdown-menu";
import { SidebarMenuLabel } from "@/shared/ui/sidebar-menu-label";
import { ProtectedBestieSidebarEntry } from "@protected-feature-components";

type SidebarSelectedView =
  | "home"
  | "channel"
  | "messages"
  | "agents"
  | "workflows"
  | "pulse"
  | "projects"
  | "people";

type AppSidebarPinnedHeaderProps = {
  channelLabels: Record<string, string>;
  currentChannelId?: string | null;
  currentPubkey?: string;
  onBrowseChannels?: () => void;
  onCreateAgent: () => void;
  onCreateChannel: () => void;
  onOpenDm: (input: { pubkeys: string[] }) => Promise<void>;
  onOpenSearchResult: (hit: SearchHit, query: string) => void;
  onSelectChannel: (channelId: string) => void;
  searchChannels: Channel[];
  searchFocusRequest: number;
  scopeSearchFocusRequest: number;
  suggestionChannels: Channel[];
};

type AppSidebarPrimaryMenuProps = {
  homeBadgeCount: number;
  inboxActive?: boolean;
  onSelectAgents: () => void;
  onSelectChats?: () => void;
  onSelectHome: () => void;
  onSelectPeople?: () => void;
  onSelectProjects: () => void;
  onSelectPulse: () => void;
  onSelectWorkflows: () => void;
  projectsOverviewActive: boolean;
  selectedView: SidebarSelectedView;
};

export function AppSidebarPinnedHeader({
  channelLabels,
  currentChannelId,
  currentPubkey,
  onBrowseChannels,
  onCreateAgent,
  onCreateChannel,
  onOpenDm,
  onOpenSearchResult,
  onSelectChannel,
  searchChannels,
  searchFocusRequest,
  scopeSearchFocusRequest,
  suggestionChannels,
}: AppSidebarPinnedHeaderProps) {
  return (
    <div
      className="mx-[3px] shrink-0 px-2 pb-2 pt-3"
      data-testid="sidebar-pinned-header"
    >
      <TopbarSearch
        channelLabels={channelLabels}
        channels={searchChannels}
        currentChannelId={currentChannelId}
        currentPubkey={currentPubkey}
        focusRequest={searchFocusRequest}
        onOpenChannel={onSelectChannel}
        onOpenResult={onOpenSearchResult}
        onOpenUser={(user) => onOpenDm({ pubkeys: [user.pubkey] })}
        onBrowseChannels={onBrowseChannels}
        onCreateAgent={onCreateAgent}
        onCreateChannel={onCreateChannel}
        scopeFocusRequest={scopeSearchFocusRequest}
        suggestionChannels={suggestionChannels}
      />
    </div>
  );
}

export function AppSidebarPrimaryMenu({
  homeBadgeCount,
  inboxActive = false,
  onSelectAgents,
  onSelectChats,
  onSelectHome,
  onSelectPeople,
  onSelectProjects,
  onSelectPulse,
  onSelectWorkflows,
  projectsOverviewActive,
  selectedView,
}: AppSidebarPrimaryMenuProps) {
  const activityActive = selectedView === "home" || inboxActive;
  return (
    <>
      <SidebarHeader
        className="relative z-40 cursor-default select-none px-2 pb-0 pt-0"
        data-tauri-drag-region
        data-testid="sidebar-primary-menu"
      >
        <SidebarMenu className="sidebar-primary-menu pb-2">
          {onSelectChats ? (
            <SidebarMenuItem>
              <SidebarMenuButton
                className="data-[active=true]:font-normal"
                data-testid="open-chats-view"
                isActive={selectedView === "channel" && !inboxActive}
                onClick={onSelectChats}
                tooltip="Chats"
                type="button"
              >
                <MessagesSquare className="h-4 w-4" />
                <SidebarMenuLabel>Chats</SidebarMenuLabel>
              </SidebarMenuButton>
            </SidebarMenuItem>
          ) : null}
          {onSelectPeople ? (
            <SidebarMenuItem>
              <SidebarMenuButton
                className="data-[active=true]:font-normal"
                data-testid="open-people-view"
                isActive={selectedView === "people"}
                onClick={onSelectPeople}
                tooltip="People"
                type="button"
              >
                <Users className="h-4 w-4" />
                <SidebarMenuLabel>People</SidebarMenuLabel>
              </SidebarMenuButton>
            </SidebarMenuItem>
          ) : null}
          <SidebarMenuItem>
            <SidebarMenuButton
              className="data-[active=true]:font-normal"
              data-testid="open-activity-view"
              isActive={activityActive}
              onClick={onSelectHome}
              tooltip="Activity"
              type="button"
            >
              <Inbox className="h-4 w-4" />
              <SidebarMenuLabel>Activity</SidebarMenuLabel>
            </SidebarMenuButton>
            {homeBadgeCount > 0 ? (
              <SidebarMenuBadge
                className="right-2 rounded-full bg-primary/15 px-1.5 text-2xs text-primary peer-data-[active=true]/menu-button:bg-sidebar-active-foreground/20 peer-data-[active=true]/menu-button:text-sidebar-active-foreground"
                data-testid="sidebar-home-count"
              >
                {Math.min(homeBadgeCount, 99)}
              </SidebarMenuBadge>
            ) : null}
          </SidebarMenuItem>
          <SidebarMenuItem>
            <SidebarMenuButton
              className="data-[active=true]:font-normal"
              data-testid="open-agents-view"
              isActive={selectedView === "agents"}
              onClick={onSelectAgents}
              tooltip="Agents"
              type="button"
            >
              <Bot className="h-4 w-4" />
              <SidebarMenuLabel>Agents</SidebarMenuLabel>
            </SidebarMenuButton>
          </SidebarMenuItem>
          <ProtectedBestieSidebarEntry />
          <AppSidebarMoreMenu
            onSelectProjects={onSelectProjects}
            onSelectPulse={onSelectPulse}
            onSelectWorkflows={onSelectWorkflows}
            projectsOverviewActive={projectsOverviewActive}
            selectedView={selectedView}
          />
        </SidebarMenu>
      </SidebarHeader>
      <SidebarProjectsSection />
    </>
  );
}

/** Pulse, Projects and Workflows live behind one "More" rail item. */
function AppSidebarMoreMenu({
  onSelectProjects,
  onSelectPulse,
  onSelectWorkflows,
  projectsOverviewActive,
  selectedView,
}: Pick<
  AppSidebarPrimaryMenuProps,
  | "onSelectProjects"
  | "onSelectPulse"
  | "onSelectWorkflows"
  | "projectsOverviewActive"
  | "selectedView"
>) {
  const pulseEnabled = useFeatureEnabled("pulse");
  const projectsEnabled = useFeatureEnabled("projects");
  const workflowsEnabled = useFeatureEnabled("workflows");
  if (!pulseEnabled && !projectsEnabled && !workflowsEnabled) {
    return null;
  }
  const moreActive =
    selectedView === "pulse" ||
    selectedView === "workflows" ||
    (selectedView === "projects" && projectsOverviewActive);
  return (
    <SidebarMenuItem>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <SidebarMenuButton
            className="data-[active=true]:font-normal"
            data-testid="open-more-menu"
            isActive={moreActive}
            tooltip="More"
            type="button"
          >
            <Ellipsis className="h-4 w-4" />
            <SidebarMenuLabel>More</SidebarMenuLabel>
          </SidebarMenuButton>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" side="right">
          {pulseEnabled ? (
            <DropdownMenuItem
              data-testid="open-pulse-view"
              onSelect={onSelectPulse}
            >
              <Activity className="h-4 w-4" />
              <span>Pulse</span>
            </DropdownMenuItem>
          ) : null}
          {projectsEnabled ? (
            <DropdownMenuItem
              data-testid="open-projects-view"
              onSelect={onSelectProjects}
            >
              <Folders className="h-4 w-4" />
              <span>Projects</span>
            </DropdownMenuItem>
          ) : null}
          {workflowsEnabled ? (
            <DropdownMenuItem
              data-testid="open-workflows-view"
              onSelect={onSelectWorkflows}
            >
              <Zap className="h-4 w-4" />
              <span>Workflows</span>
            </DropdownMenuItem>
          ) : null}
        </DropdownMenuContent>
      </DropdownMenu>
    </SidebarMenuItem>
  );
}
