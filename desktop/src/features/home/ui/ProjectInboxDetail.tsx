import { ArrowLeft } from "lucide-react";

import { useAppNavigation } from "@/app/navigation/useAppNavigation";
import type { InboxItem } from "@/features/home/lib/inbox";
import { resolveProjectInboxWorkItem } from "@/features/home/lib/projectInbox";
import { ProjectInboxDetailPane } from "@/features/home/ui/ProjectInboxDetailPane";
import {
  useProjectsQuery,
  useProjectsWorkItemsQuery,
} from "@/features/projects/hooks";
import type { UserProfileLookup } from "@/features/profile/lib/identity";
import { useT } from "@/shared/i18n";
import { Button } from "@/shared/ui/button";

type ProjectInboxDetailProps = {
  isSinglePanelView?: boolean;
  item: InboxItem;
  onBack?: () => void;
  profiles?: UserProfileLookup;
};

function ProjectInboxStatus({
  message,
  onBack,
  onRetry,
}: {
  message: string;
  onBack?: () => void;
  onRetry?: () => void;
}) {
  const t = useT();
  return (
    <section className="flex min-h-0 min-w-0 flex-col bg-background/60">
      {onBack ? (
        <div className="flex min-h-13 items-center px-5 py-2">
          <Button
            aria-label={t("home.project.backToInbox")}
            onClick={onBack}
            size="icon"
            type="button"
            variant="ghost"
          >
            <ArrowLeft className="h-4 w-4" />
          </Button>
        </div>
      ) : null}
      <div className="flex min-h-0 flex-1 flex-col items-center justify-center gap-3 p-6 text-center">
        <p className="text-sm text-muted-foreground">{message}</p>
        {onRetry ? (
          <Button onClick={onRetry} size="sm" type="button" variant="outline">
            {t("common.retry")}
          </Button>
        ) : null}
      </div>
    </section>
  );
}

/** Resolves and renders the live Kuzz Git object selected from Inbox. */
export function ProjectInboxDetail({
  isSinglePanelView = false,
  item,
  onBack,
  profiles,
}: ProjectInboxDetailProps) {
  const t = useT();
  const { goProject } = useAppNavigation();
  const projectsQuery = useProjectsQuery();
  const projectsWorkItemsQuery = useProjectsWorkItemsQuery(
    projectsQuery.data ?? [],
  );
  const workItem = resolveProjectInboxWorkItem(
    item.item,
    projectsWorkItemsQuery.data,
  );

  if (!workItem) {
    const error = projectsQuery.error ?? projectsWorkItemsQuery.error;
    const isLoading =
      projectsQuery.isLoading || projectsWorkItemsQuery.isLoading;
    return (
      <ProjectInboxStatus
        message={
          error
            ? t("home.project.loadError")
            : isLoading
              ? t("home.project.loading")
              : t("home.project.notFound")
        }
        onBack={onBack}
        onRetry={
          error
            ? () => {
                void projectsQuery.refetch();
                void projectsWorkItemsQuery.refetch();
              }
            : undefined
        }
      />
    );
  }

  const failedSections =
    workItem.type === "pull-request"
      ? projectsWorkItemsQuery.data?.pullRequests.failedSections
      : projectsWorkItemsQuery.data?.issues.failedSections;
  if (failedSections && failedSections.length > 0) {
    return (
      <ProjectInboxStatus
        message={t("home.project.partialLoadError")}
        onBack={onBack}
        onRetry={() => void projectsWorkItemsQuery.refetch()}
      />
    );
  }

  return (
    <ProjectInboxDetailPane
      isSinglePanelView={isSinglePanelView}
      onBack={onBack}
      onOpenProject={() => {
        const workItemId =
          workItem.type === "pull-request"
            ? { pullRequestId: workItem.pullRequest.id }
            : { issueId: workItem.issue.id };
        void goProject(workItem.project.id, {
          ...workItemId,
          repositoryId: workItem.repository.id,
        });
      }}
      profiles={profiles}
      workItem={workItem}
    />
  );
}
