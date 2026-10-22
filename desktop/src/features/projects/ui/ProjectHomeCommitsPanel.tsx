import type { ProjectPullRequest, Repository } from "@/features/projects/hooks";
import { AlertTriangle } from "lucide-react";
import {
  projectRepoUnavailablePresentation,
  projectRepoUnavailableReason,
} from "@/features/projects/lib/projectRepoAvailability";
import type { ViewerGitIdentity } from "@/features/projects/lib/projectContributorMatching";
import type { ProjectRepositorySnapshotResult } from "@/features/projects/useProjectRepositorySnapshots";
import type { UserProfileLookup } from "@/features/profile/lib/identity";
import type { ProjectRepoCommit } from "@/shared/api/types";
import { KuzzLoadingState } from "@/shared/ui/KuzzLoadingState";
import { ProjectPanelState } from "./ProjectPanelState";
import { ActivityPanel } from "./ProjectDetailFeedPanels";
import { useT } from "@/shared/i18n";

export function ProjectHomeCommitsPanel({
  onSelectCommit,
  profiles,
  projectId,
  pullRequests,
  results,
  viewerGitIdentity,
}: {
  onSelectCommit: (commit: ProjectRepoCommit, repository: Repository) => void;
  profiles?: UserProfileLookup;
  projectId: string;
  pullRequests: ProjectPullRequest[];
  results: ProjectRepositorySnapshotResult[];
  viewerGitIdentity?: ViewerGitIdentity | null;
}) {
  const t = useT();
  const loaded = results.filter(
    (result) => (result.snapshot?.commits.length ?? 0) > 0,
  );
  const commitItems = loaded
    .flatMap(({ repository, snapshot }) =>
      (snapshot?.commits ?? []).map((commit) => ({
        branch: repository.defaultBranch,
        commit,
        project: repository,
        projectId,
        pullRequests,
        repoContributors: snapshot?.contributors ?? [],
      })),
    )
    .sort((left, right) => right.commit.timestamp - left.commit.timestamp);
  const failed = results.filter((result) => result.error);
  const firstFailure = failed[0];
  const failure = firstFailure
    ? projectRepoUnavailablePresentation(
        projectRepoUnavailableReason(firstFailure.error),
      )
    : null;
  if (results.some((result) => result.isLoading) && loaded.length === 0) {
    return <KuzzLoadingState label={t("projects.homeCommits.loading")} />;
  }
  if (loaded.length === 0) {
    return (
      <ProjectPanelState
        description={
          failure
            ? t("projects.homeCommits.failed", {
                description: failure.description,
                others: failed.length - 1,
                repository: firstFailure.repository.name,
              })
            : t("projects.homeCommits.emptyDescription")
        }
        error={failed.length > 0}
        title={failure?.title ?? t("projects.homeCommits.emptyTitle")}
      />
    );
  }

  const firstItem = commitItems[0];
  if (!firstItem) return null;
  return (
    <div className="space-y-3">
      {failed.length > 0 ? (
        <div
          className="mx-4 flex items-start gap-2 rounded-md border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-sm"
          data-testid="project-home-commits-degraded"
          role="status"
        >
          <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0 text-amber-600" />
          <p>
            {t("projects.homeCommits.partial", {
              failed: failed.length,
              loaded: loaded.length,
              total: results.length,
            })}
          </p>
        </div>
      ) : null}
      <ActivityPanel
        commitItems={commitItems}
        error={null}
        isLoading={false}
        onSelectCommit={onSelectCommit}
        profiles={profiles}
        project={firstItem.project}
        projectId={projectId}
        pullRequests={pullRequests}
        repoContributors={firstItem.repoContributors}
        snapshot={null}
        viewerGitIdentity={viewerGitIdentity}
      />
    </div>
  );
}
