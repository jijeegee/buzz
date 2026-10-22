import * as React from "react";
import {
  Bot,
  Ellipsis,
  Folders,
  MessagesSquare,
  Users,
  Zap,
} from "lucide-react";

import type { SidebarListTab } from "@/features/sidebar/lib/sidebarListTab";
import { useFeatureEnabled } from "@/shared/features";
import { useT } from "@/shared/i18n";
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
  /** The list shown in the left column; its tab is the highlighted one. */
  listTab: SidebarListTab;
  onSelectAgents: () => void;
  onSelectChats: () => void;
  onSelectPeople: () => void;
  onSelectProjects: () => void;
  onSelectWorkflows: () => void;
  /** The signed-in profile button, pinned to the bottom of the rail. */
  profileSlot?: React.ReactNode;
  selectedView: NavRailView;
};

/**
 * The app's main tabs as a narrow column of icons with labels, left of the
 * list: Chats · People · Agents · More (Projects, Workflows). Activity and
 * Pulse are hidden for now. The list beside it follows the selected tab. The profile
 * button sits at the bottom, apart from the list.
 */
export function AppNavRail({
  listTab,
  onSelectAgents,
  onSelectChats,
  onSelectPeople,
  onSelectProjects,
  onSelectWorkflows,
  profileSlot,
  selectedView,
}: AppNavRailProps) {
  // The workflows screen has no list; its More item stays lit while open.
  const onWorkflows = selectedView === "workflows";
  const t = useT();
  return (
    <nav
      aria-label={t("nav.label")}
      className="relative z-20 flex w-16 shrink-0 select-none flex-col items-center gap-1 overflow-y-auto bg-sidebar px-1.5 pb-5 pt-[calc(var(--kuzz-top-chrome-height,40px)+4px)]"
      data-testid="sidebar-primary-menu"
    >
      <RailButton
        active={!onWorkflows && listTab === "chats"}
        icon={<MessagesSquare className="h-5 w-5" />}
        label={t("nav.chats")}
        onClick={onSelectChats}
        testId="open-chats-view"
      />
      <RailButton
        active={!onWorkflows && listTab === "people"}
        icon={<Users className="h-5 w-5" />}
        label={t("nav.friends")}
        onClick={onSelectPeople}
        testId="open-people-view"
      />
      <RailButton
        active={!onWorkflows && listTab === "agents"}
        icon={<Bot className="h-5 w-5" />}
        label={t("nav.agents")}
        onClick={onSelectAgents}
        testId="open-agents-view"
      />
      <RailMoreMenu
        onSelectProjects={onSelectProjects}
        onSelectWorkflows={onSelectWorkflows}
        active={onWorkflows || listTab === "projects"}
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

/** Projects and Workflows live behind one "More" rail item. */
function RailMoreMenu({
  active,
  onSelectProjects,
  onSelectWorkflows,
}: Pick<AppNavRailProps, "onSelectProjects" | "onSelectWorkflows"> & {
  active: boolean;
}) {
  const projectsEnabled = useFeatureEnabled("projects");
  const workflowsEnabled = useFeatureEnabled("workflows");
  const t = useT();
  if (!projectsEnabled && !workflowsEnabled) {
    return null;
  }
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <RailButton
          active={active}
          icon={<Ellipsis className="h-5 w-5" />}
          label={t("nav.more")}
          testId="open-more-menu"
        />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" side="right">
        {projectsEnabled ? (
          <DropdownMenuItem
            data-testid="open-projects-view"
            onSelect={onSelectProjects}
          >
            <Folders className="h-4 w-4" />
            <span>{t("nav.projects")}</span>
          </DropdownMenuItem>
        ) : null}
        {workflowsEnabled ? (
          <DropdownMenuItem
            data-testid="open-workflows-view"
            onSelect={onSelectWorkflows}
          >
            <Zap className="h-4 w-4" />
            <span>{t("nav.workflows")}</span>
          </DropdownMenuItem>
        ) : null}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
