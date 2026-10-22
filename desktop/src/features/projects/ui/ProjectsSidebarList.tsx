import * as React from "react";
import { useLocation } from "@tanstack/react-router";
import { Folders, LayoutGrid } from "lucide-react";

import { useAppNavigation } from "@/app/navigation/useAppNavigation";
import { useProjectsQuery } from "@/features/projects/hooks";
import { projectMatchesRouteId } from "@/features/projects/projectRoutes";
import { selectedProjectRouteId } from "@/features/sidebar/ui/listSidebarProjects";
import { cn } from "@/shared/lib/cn";
import { Input } from "@/shared/ui/input";
import { Skeleton } from "@/shared/ui/skeleton";
import { useT } from "@/shared/i18n";

/**
 * The Projects tab's list, in the left column: an Overview row, then every
 * project by name. Picking one opens it on the right.
 */
export function ProjectsSidebarList() {
  const t = useT();
  const { goProject, goProjects } = useAppNavigation();
  const pathname = useLocation({ select: (location) => location.pathname });
  const routeProjectId = selectedProjectRouteId(pathname);
  const projectsQuery = useProjectsQuery();
  const [filter, setFilter] = React.useState("");
  const needle = filter.trim().toLowerCase();
  const projects = React.useMemo(
    () =>
      [...(projectsQuery.data ?? [])]
        .filter(
          (project) =>
            needle.length === 0 || project.name.toLowerCase().includes(needle),
        )
        .sort((a, b) =>
          a.name.localeCompare(b.name, undefined, { sensitivity: "base" }),
        ),
    [needle, projectsQuery.data],
  );

  return (
    <div className="flex min-h-0 flex-1 flex-col" data-testid="projects-list">
      <div className="space-y-2 px-2 pb-2 pt-1">
        <h2 className="px-1 text-base font-semibold text-sidebar-foreground">
          {t("projects.sidebar.title")}
        </h2>
        <Input
          aria-label={t("projects.sidebar.filterLabel")}
          className="h-8 bg-background"
          data-testid="projects-filter"
          onChange={(event) => setFilter(event.target.value)}
          placeholder={t("projects.sidebar.filterPlaceholder")}
          value={filter}
        />
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto overflow-x-hidden px-1 pb-4">
        <ListRow
          icon={<LayoutGrid className="h-4 w-4 text-muted-foreground" />}
          isSelected={pathname === "/projects"}
          onClick={() => void goProjects()}
          testId="projects-list-overview"
          title={t("projects.sidebar.overview")}
        />
        {projectsQuery.isLoading ? (
          <div className="space-y-3 px-2 py-2">
            {[1, 2, 3].map((i) => (
              <Skeleton className="h-8 w-full" key={i} />
            ))}
          </div>
        ) : projects.length > 0 ? (
          projects.map((project) => (
            <ListRow
              icon={<Folders className="h-4 w-4 text-muted-foreground" />}
              isSelected={
                routeProjectId != null &&
                projectMatchesRouteId(project, routeProjectId)
              }
              key={project.id}
              onClick={() => void goProject(project.id)}
              subtitle={project.description}
              testId={`projects-list-row-${project.dtag}`}
              title={project.name}
            />
          ))
        ) : (
          <p className="px-2 py-3 text-xs text-sidebar-foreground/60">
            {needle
              ? t("projects.sidebar.noMatch")
              : t("projects.sidebar.empty")}
          </p>
        )}
      </div>
    </div>
  );
}

function ListRow({
  icon,
  isSelected,
  onClick,
  subtitle,
  testId,
  title,
}: {
  icon: React.ReactNode;
  isSelected: boolean;
  onClick: () => void;
  subtitle?: string;
  testId: string;
  title: string;
}) {
  return (
    <button
      aria-current={isSelected || undefined}
      className={cn(
        "flex min-h-12 w-full items-center gap-2.5 rounded-md px-2 py-1.5 text-left text-sm text-sidebar-foreground outline-hidden transition-colors hover:bg-sidebar-accent focus-visible:ring-2 focus-visible:ring-sidebar-ring",
        isSelected && "bg-sidebar-accent",
      )}
      data-testid={testId}
      onClick={onClick}
      type="button"
    >
      <span className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-muted/60">
        {icon}
      </span>
      <span className="flex min-w-0 flex-1 flex-col">
        <span className="truncate font-medium">{title}</span>
        {subtitle?.trim() ? (
          <span className="truncate text-xs text-sidebar-foreground/55">
            {subtitle.trim()}
          </span>
        ) : null}
      </span>
    </button>
  );
}
