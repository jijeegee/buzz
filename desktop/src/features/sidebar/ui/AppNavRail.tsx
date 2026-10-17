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
  inboxActive?: boolean;
  onSelectAgents: () => void;
  onSelectChats: () => void;
  onSelectHome: () => void;
  onSelectPeople: () => void;
  onSelectProjects: () => void;
  onSelectPulse: () => void;
  onSelectWorkflows: () => void;
  projectsOverviewActive: boolean;
  selectedView: NavRailView;
};

/**
 * The app's main tabs as a narrow column of icons with labels, left of the
 * list: Chats · People · Activity · Agents · More (Pulse, Projects,
 * Workflows). The list beside it follows the selected tab.
 */
export function AppNavRail({
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
}: AppNavRailProps) {
  const activityActive = selectedView === "home" || inboxActive;
  return (
    <nav
      aria-label="Main"
      className="relative z-20 flex w-16 shrink-0 select-none flex-col items-center gap-1 overflow-y-auto bg-sidebar px-1.5 pb-5 pt-[calc(var(--kuzz-top-chrome-height,40px)+4px)]"
      data-testid="sidebar-primary-menu"
    >
      <RailButton
        active={selectedView === "channel" && !inboxActive}
        icon={<MessagesSquare className="h-5 w-5" />}
        label="Chats"
        onClick={onSelectChats}
        testId="open-chats-view"
      />
      <RailButton
        active={selectedView === "people"}
        icon={<Users className="h-5 w-5" />}
        label="People"
        onClick={onSelectPeople}
        testId="open-people-view"
      />
      <RailButton
        active={activityActive}
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
        active={selectedView === "agents"}
        icon={<Bot className="h-5 w-5" />}
        label="Agents"
        onClick={onSelectAgents}
        testId="open-agents-view"
      />
      <RailMoreMenu
        onSelectProjects={onSelectProjects}
        onSelectPulse={onSelectPulse}
        onSelectWorkflows={onSelectWorkflows}
        projectsOverviewActive={projectsOverviewActive}
        selectedView={selectedView}
      />
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
  onSelectProjects,
  onSelectPulse,
  onSelectWorkflows,
  projectsOverviewActive,
  selectedView,
}: Pick<
  AppNavRailProps,
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
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <RailButton
          active={moreActive}
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
