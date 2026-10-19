import * as React from "react";
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

import type { SidebarListTab } from "@/features/sidebar/lib/sidebarListTab";
import { useFeatureEnabled } from "@/shared/features";
import { cn } from "@/shared/lib/cn";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/shared/ui/dropdown-menu";

export type NavRailView =
  | "home"
  | "channel"
  | "messages"
  | "agents"
  | "workflows"
  | "pulse"
  | "projects"
  | "people";

type AppNavRailProps = {
  homeBadgeCount: number;
  /** The list shown in the left column; its tab is the highlighted one. */
  listTab: SidebarListTab;
  onSelectAgents: () => void;
  onSelectChats: () => void;
  onSelectHome: () => void;
  onSelectPeople: () => void;
  onSelectProjects: () => void;
  onSelectPulse: () => void;
  onSelectWorkflows: () => void;
  /** The signed-in profile button, pinned to the bottom of the rail. */
  profileSlot?: React.ReactNode;
  selectedView: NavRailView;
};

/**
 * The app's main tabs as a narrow column of icons with labels, left of the
 * list: Chats · People · Activity · Agents · More (Pulse, Projects,
 * Workflows). The list beside it follows the selected tab. The profile
 * button sits at the bottom, apart from the list.
 */
export function AppNavRail({
  homeBadgeCount,
  listTab,
  onSelectAgents,
  onSelectChats,
  onSelectHome,
  onSelectPeople,
  onSelectProjects,
  onSelectPulse,
  onSelectWorkflows,
  profileSlot,
  selectedView,
}: AppNavRailProps) {
  // The workflows screen has no list; its More item stays lit while open.
  const onWorkflows = selectedView === "workflows";
  return (
    <nav
      aria-label="Main"
      className="relative z-20 flex w-16 shrink-0 select-none flex-col items-center gap-1 overflow-y-auto bg-sidebar px-1.5 pb-5 pt-[calc(var(--kuzz-top-chrome-height,40px)+4px)]"
      data-testid="sidebar-primary-menu"
    >
      <RailButton
        active={!onWorkflows && listTab === "chats"}
        icon={<MessagesSquare className="h-5 w-5" />}
        label="Chats"
        onClick={onSelectChats}
        testId="open-chats-view"
      />
      <RailButton
        active={!onWorkflows && listTab === "people"}
        icon={<Users className="h-5 w-5" />}
        label="People"
        onClick={onSelectPeople}
        testId="open-people-view"
      />
      <RailButton
        active={!onWorkflows && listTab === "activity"}
        badge={
          homeBadgeCount > 0 ? (
            <span
              className="absolute right-1.5 top-0.5 flex h-4 min-w-4 items-center justify-center rounded-full bg-primary px-1 text-2xs font-semibold leading-none text-primary-foreground tabular-nums"
              data-testid="sidebar-home-count"
            >
              {Math.min(homeBadgeCount, 99)}
            </span>
          ) : null
        }
        icon={<Inbox className="h-5 w-5" />}
        label="Activity"
        onClick={onSelectHome}
        testId="open-activity-view"
      />
      <RailButton
        active={!onWorkflows && listTab === "agents"}
        icon={<Bot className="h-5 w-5" />}
        label="Agents"
        onClick={onSelectAgents}
        testId="open-agents-view"
      />
      <RailMoreMenu
        onSelectProjects={onSelectProjects}
        onSelectPulse={onSelectPulse}
        onSelectWorkflows={onSelectWorkflows}
        active={onWorkflows || listTab === "projects" || listTab === "pulse"}
      />
      {profileSlot ? (
        <div className="mt-auto w-full pt-2">{profileSlot}</div>
      ) : null}
    </nav>
  );
}

const RAIL_BUTTON_CLASS =
  "relative flex w-full flex-col items-center gap-0.5 rounded-lg px-1 py-1.5 text-2xs font-medium text-sidebar-foreground/70 outline-hidden transition-colors hover:bg-sidebar-accent hover:text-sidebar-foreground focus-visible:ring-1 focus-visible:ring-ring";
const RAIL_BUTTON_ACTIVE_CLASS =
  "bg-sidebar-active text-sidebar-active-foreground hover:bg-sidebar-active hover:text-sidebar-active-foreground";

type RailButtonProps = {
  active: boolean;
  badge?: React.ReactNode;
  icon: React.ReactNode;
  label: string;
  testId: string;
} & React.ButtonHTMLAttributes<HTMLButtonElement>;

// Forwards its ref so it can be a dropdown trigger (More).
const RailButton = React.forwardRef<HTMLButtonElement, RailButtonProps>(
  function RailButton(
    { active, badge, className, icon, label, testId, ...rest },
    ref,
  ) {
    return (
      <button
        {...rest}
        aria-current={active ? "page" : undefined}
        className={cn(
          RAIL_BUTTON_CLASS,
          active && RAIL_BUTTON_ACTIVE_CLASS,
          className,
        )}
        data-active={active}
        data-testid={testId}
        ref={ref}
        type="button"
      >
        {icon}
        <span className="max-w-full truncate leading-tight">{label}</span>
        {badge}
      </button>
    );
  },
);

/** Pulse, Projects and Workflows live behind one "More" rail item. */
function RailMoreMenu({
  active,
  onSelectProjects,
  onSelectPulse,
  onSelectWorkflows,
}: Pick<
  AppNavRailProps,
  "onSelectProjects" | "onSelectPulse" | "onSelectWorkflows"
> & { active: boolean }) {
  const pulseEnabled = useFeatureEnabled("pulse");
  const projectsEnabled = useFeatureEnabled("projects");
  const workflowsEnabled = useFeatureEnabled("workflows");
  if (!pulseEnabled && !projectsEnabled && !workflowsEnabled) {
    return null;
  }
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <RailButton
          active={active}
          icon={<Ellipsis className="h-5 w-5" />}
          label="More"
          testId="open-more-menu"
        />
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
  );
}
