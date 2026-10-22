import { Eye, FolderKanban } from "lucide-react";
import * as React from "react";

import type {
  Project,
  ProjectIssue,
  ProjectIssueListItem,
  Repository,
} from "@/features/projects/hooks";
import { issueShareLink } from "@/features/projects/lib/projectShareLinks";
import { issueStatusLabel } from "@/features/projects/lib/projectStatusLabels";
import { selectionItemFromTask } from "@/features/projects/lib/projectSelection";
import type { ProjectWorkItemSection } from "@/features/projects/projectWorkItems";
import {
  resolveUserLabel,
  type UserProfileLookup,
} from "@/features/profile/lib/identity";
import {
  countGroupedRows,
  sliceGroupedRows,
  useIncrementalMount,
} from "@/shared/hooks/useIncrementalMount";
import { cn } from "@/shared/lib/cn";
import { KuzzLoadingState } from "@/shared/ui/KuzzLoadingState";
import { Button } from "@/shared/ui/button";
import { Card } from "@/shared/ui/card";
import { DropdownMenuItem } from "@/shared/ui/dropdown-menu";
import { CopyShareLinkMenuItem } from "./CopyShareLinkMenuItem";
import { ProjectAuthorIdentity } from "./ProjectAuthorIdentity";
import { ProjectEntityListRow } from "./ProjectEntityListRow";
import { ProjectEventTypeIcon } from "./ProjectEventTypeIcon";
import { PROJECT_GRID_CARD_BODY_CLASS } from "./projectGridCardStyles";
import { ProjectListRowMenu } from "./ProjectListRowMenu";
import { ProjectSelectableGroup } from "./ProjectSelectableGroup";
import { ProjectPanelState } from "./ProjectPanelState";
import { ProjectsWorkItemsLoadNotice } from "./ProjectsWorkItemsLoadNotice";
import { groupProjectWorkItemsByProject } from "./projectWorkItemGroups";
import { type Translate, useT } from "@/shared/i18n";

type ProjectsIssuesListProps = {
  /** Render without container chrome — a parent table container provides border and rounding. */
  embedded?: boolean;
  emptyMessage?: string;
  error: unknown;
  failedSections: ProjectWorkItemSection[];
  isLoading: boolean;
  isRetrying: boolean;
  onOpen: (
    project: Project,
    repository: Repository,
    issue: ProjectIssue,
  ) => void;
  onRetry: () => void;
  profiles?: UserProfileLookup;
  issues: ProjectIssueListItem[];
  viewMode: "grid" | "list";
};

function nextStepLabel(status: ProjectIssue["status"], t: Translate) {
  if (status === "Done" || status === "Closed")
    return t("projects.tasksList.viewTask");
  if (status === "In Review") return t("projects.tasksList.reviewTask");
  if (status === "Triage") return t("projects.tasksList.triageTask");
  return t("projects.tasksList.openTask");
}

const IssueGridCard = React.memo(function IssueGridCard({
  issue,
  onOpen,
  project,
  repository,
}: {
  issue: ProjectIssue;
  onOpen: (
    project: Project,
    repository: Repository,
    issue: ProjectIssue,
  ) => void;
  project: Project;
  repository: Repository;
}) {
  const t = useT();
  return (
    <Card
      className="group relative flex min-h-32 flex-col overflow-hidden border-border/60 bg-transparent p-4 shadow-none transition-colors duration-150 hover:bg-muted/20"
      data-projects-grid-card
    >
      <button
        className="absolute inset-0"
        onClick={() => onOpen(project, repository, issue)}
        type="button"
      >
        <span className="sr-only">
          {t("projects.tasksList.viewItem", { title: issue.title })}
        </span>
      </button>
      <div className="flex min-h-0 flex-1 flex-col gap-2">
        <h3
          className="truncate text-sm font-semibold leading-5 text-foreground"
          data-testid="projects-grid-card-title"
        >
          {issue.title}
        </h3>
        <p
          className={cn(PROJECT_GRID_CARD_BODY_CLASS, "text-muted-foreground")}
          data-testid="projects-grid-card-body"
        >
          {issue.content || t("projects.list.noDescription")}
        </p>
        <div
          className="mt-auto flex items-center gap-1.5 text-xs font-medium text-muted-foreground"
          data-testid="projects-grid-card-indicator"
        >
          <ProjectEventTypeIcon className="h-3.5 w-3.5" kind="issue" />
          <span>{issueStatusLabel(issue.status)}</span>
        </div>
      </div>
    </Card>
  );
});

function issueSelectionItem(
  project: Project,
  repository: Repository,
  issue: ProjectIssue,
) {
  return selectionItemFromTask({
    author: issue.author,
    channelId: repository.channelId ?? project.projectChannelId,
    id: issue.id,
    shareLink: issueShareLink(issue),
    title: issue.title,
  });
}

// Memoized: these rows render in unbounded lists, and any Projects-view
// state change used to re-render every row. Props are kept identity-stable
// by the list (memoized groups/selection arrays, stable onOpen).
const IssueListRow = React.memo(function IssueListRow({
  issue,
  onOpen,
  profiles,
  project,
  rangeItems,
  repository,
}: {
  issue: ProjectIssue;
  onOpen: (
    project: Project,
    repository: Repository,
    issue: ProjectIssue,
  ) => void;
  profiles?: UserProfileLookup;
  project: Project;
  rangeItems: ReturnType<typeof issueSelectionItem>[];
  repository: Repository;
}) {
  const t = useT();
  const authorLabel = resolveUserLabel({ profiles, pubkey: issue.author });

  return (
    <ProjectEntityListRow
      affiliation={repository.name}
      count={issue.comments.length}
      dateSeconds={issue.updatedAt}
      dateTestId="projects-row-date"
      icon={null}
      onClick={() => onOpen(project, repository, issue)}
      peopleSlot={
        <ProjectAuthorIdentity
          label={authorLabel}
          labelClassName="sr-only"
          profiles={profiles}
          pubkey={issue.author}
          testId="projects-issue-author"
        />
      }
      selection={{
        item: issueSelectionItem(project, repository, issue),
        rangeItems,
      }}
      testId={`projects-issue-row-${issue.id}`}
      title={issue.title}
      titleAttr={t("projects.tasksList.openItem", { title: issue.title })}
      titleIcon={<ProjectEventTypeIcon className="h-3.5 w-3.5" kind="issue" />}
      trailing={
        <ProjectListRowMenu
          label={t("projects.cards.moreOptions", { name: issue.title })}
        >
          <DropdownMenuItem onSelect={() => onOpen(project, repository, issue)}>
            <Eye className="h-4 w-4" />
            {nextStepLabel(issue.status, t)}
          </DropdownMenuItem>
          <CopyShareLinkMenuItem
            link={issueShareLink(issue)}
            label={t("projects.issues.copyLink")}
            testId={`projects-issue-copy-link-${issue.id}`}
          />
        </ProjectListRowMenu>
      }
    />
  );
});

export function ProjectsIssuesList({
  embedded,
  emptyMessage,
  error,
  failedSections,
  isLoading,
  isRetrying,
  issues,
  onOpen,
  onRetry,
  profiles,
  viewMode,
}: ProjectsIssuesListProps) {
  const t = useT();
  // Grouping and per-group selection arrays are identity-stable across
  // re-renders so the memoized rows only re-render when their data changes.
  const allGroups = React.useMemo(
    () =>
      groupProjectWorkItemsByProject(issues).map((group) => ({
        ...group,
        selectionItems: group.rows.map((row) =>
          issueSelectionItem(row.project, row.repository, row.issue),
        ),
      })),
    [issues],
  );
  // Mount rows progressively: a one-shot mount of hundreds of rows blocked
  // the main thread for over a second on tab entry.
  // Each layout's counter grows only while that layout is active: otherwise
  // a layout switch would find the other counter already grown and mount the
  // whole collection in one commit.
  const mountedRowCount = useIncrementalMount(
    countGroupedRows(allGroups),
    30,
    60,
    viewMode !== "grid",
  );
  const mountedGridCount = useIncrementalMount(
    issues.length,
    30,
    60,
    viewMode === "grid",
  );
  const groups = React.useMemo(
    () => sliceGroupedRows(allGroups, mountedRowCount),
    [allGroups, mountedRowCount],
  );

  if (isLoading) {
    return <KuzzLoadingState label={t("projects.tasksList.loading")} />;
  }

  const loadNotice = (
    <ProjectsWorkItemsLoadNotice
      error={error}
      failedSections={failedSections}
      isRetrying={isRetrying}
      onRetry={onRetry}
      subject="issues"
    />
  );

  if (error && issues.length === 0) {
    return (
      <ProjectPanelState
        action={
          <Button
            disabled={isRetrying}
            onClick={onRetry}
            size="sm"
            variant="outline"
          >
            {isRetrying ? t("projects.loadNotice.retrying") : t("common.retry")}
          </Button>
        }
        description={
          error instanceof Error
            ? error.message
            : t("projects.loadNotice.genericError")
        }
        error
        panel={false}
        title={t("projects.tasksList.errorTitle")}
      />
    );
  }

  if (issues.length === 0) {
    return (
      <div className="space-y-3">
        {loadNotice}
        <ProjectPanelState
          className={cn(!embedded && "border border-dashed border-border/60")}
          panel={false}
          title={emptyMessage ?? t("projects.tasksList.empty")}
        />
      </div>
    );
  }

  if (viewMode === "grid") {
    return (
      <div className="space-y-3">
        {loadNotice}
        <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
          {issues
            .slice(0, mountedGridCount)
            .map(({ project, issue, repository }) => (
              <IssueGridCard
                issue={issue}
                key={`${repository.id}:${issue.id}`}
                onOpen={onOpen}
                project={project}
                repository={repository}
              />
            ))}
        </div>
      </div>
    );
  }

  return (
    <div className="space-y-3">
      {loadNotice}
      <div data-testid="projects-list-container">
        {groups.map((group) => {
          const groupSelectionItems = group.selectionItems;
          return (
            <ProjectSelectableGroup
              count={group.rows.length}
              groupKey={group.project.id}
              headerClassName="mx-0 gap-3 px-4"
              headerTestId="projects-issue-project-group-header"
              icon={<FolderKanban className="h-4 w-4" />}
              items={groupSelectionItems}
              key={group.project.id}
              label={group.project.name}
              labelTestId="project-issue-project"
              testId="projects-issue-project-group"
            >
              <ul>
                {group.rows.map(({ project, issue, repository }) => (
                  <li
                    className="[contain-intrinsic-size:auto_3.5rem] [content-visibility:auto]"
                    key={`${repository.id}:${issue.id}`}
                  >
                    <IssueListRow
                      issue={issue}
                      onOpen={onOpen}
                      profiles={profiles}
                      project={project}
                      rangeItems={groupSelectionItems}
                      repository={repository}
                    />
                  </li>
                ))}
              </ul>
            </ProjectSelectableGroup>
          );
        })}
      </div>
    </div>
  );
}
