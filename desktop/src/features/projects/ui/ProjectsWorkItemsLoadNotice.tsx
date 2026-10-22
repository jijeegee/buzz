import { AlertCircle } from "lucide-react";

import type { ProjectWorkItemSection } from "@/features/projects/projectWorkItems";
import { Button } from "@/shared/ui/button";
import { type MessageKey, useT } from "@/shared/i18n";

const SECTION_LABELS: Record<ProjectWorkItemSection, MessageKey> = {
  assignments: "projects.loadNotice.sectionAssignments",
  comments: "projects.loadNotice.sectionComments",
  "pull-request-updates": "projects.loadNotice.sectionReviewUpdates",
  statuses: "projects.loadNotice.sectionStatuses",
};

const LOAD_FAILED_TITLES: Record<
  ProjectsWorkItemsLoadNoticeProps["subject"],
  MessageKey
> = {
  issues: "projects.loadNotice.tasksFailed",
  "project activity": "projects.loadNotice.activityFailed",
  "pull requests": "projects.loadNotice.reviewsFailed",
};

const PARTIAL_TITLES: Record<
  ProjectsWorkItemsLoadNoticeProps["subject"],
  MessageKey
> = {
  issues: "projects.loadNotice.tasksPartial",
  "project activity": "projects.loadNotice.activityPartial",
  "pull requests": "projects.loadNotice.reviewsPartial",
};

type ProjectsWorkItemsLoadNoticeProps = {
  error: unknown;
  failedSections: ProjectWorkItemSection[];
  isRetrying: boolean;
  onRetry: () => void;
  subject: "issues" | "project activity" | "pull requests";
};

/** Displays full and partial aggregate work-item failures with a retry action. */
export function ProjectsWorkItemsLoadNotice({
  error,
  failedSections,
  isRetrying,
  onRetry,
  subject,
}: ProjectsWorkItemsLoadNoticeProps) {
  const t = useT();
  if (!error && failedSections.length === 0) return null;

  const title = error
    ? t(LOAD_FAILED_TITLES[subject])
    : t(PARTIAL_TITLES[subject]);
  const description = error
    ? error instanceof Error
      ? error.message
      : t("projects.loadNotice.genericError")
    : t("projects.loadNotice.missing", {
        sections: failedSections
          .map((section) => t(SECTION_LABELS[section]))
          .join(", "),
      });

  return (
    <div
      className="flex items-start gap-3 border border-destructive/30 bg-destructive/5 px-4 py-3 text-sm"
      role="alert"
    >
      <AlertCircle className="mt-0.5 h-4 w-4 shrink-0 text-destructive" />
      <div className="min-w-0 flex-1">
        <p className="font-medium text-foreground">{title}</p>
        <p className="mt-0.5 text-muted-foreground">{description}</p>
      </div>
      <Button
        disabled={isRetrying}
        onClick={onRetry}
        size="sm"
        variant="outline"
      >
        {isRetrying ? t("projects.loadNotice.retrying") : t("common.retry")}
      </Button>
    </div>
  );
}
