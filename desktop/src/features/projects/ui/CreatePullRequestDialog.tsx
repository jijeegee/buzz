import * as React from "react";
import { toast } from "sonner";

import {
  type Project,
  type Repository,
  useProjectPullRequestsQuery,
  useRepoStateQuery,
} from "@/features/projects/hooks";
import { selectProjectRepository } from "@/features/projects/projectModels";
import { useCreateProjectPullRequestMutation } from "@/features/projects/pullRequestMutations";
import { useProjectRepoSyncStatusQuery } from "@/features/projects/repoSyncHooks";

import {
  CreateProjectWorkItemDialog,
  type CreateProjectWorkItemDialogInput,
} from "./CreateProjectWorkItemDialog";
import { useT } from "@/shared/i18n";

export type CreatePullRequestDialogInput = CreateProjectWorkItemDialogInput;

export function CreatePullRequestDialog({
  initialProjectId,
  onCreated,
  onOpenChange,
  open,
  projects,
  reposDir,
}: {
  initialProjectId?: string;
  onCreated: (
    project: Project,
    repository: Repository,
    pullRequestId: string,
  ) => void | Promise<void>;
  onOpenChange: (open: boolean) => void;
  open: boolean;
  projects: Project[];
  reposDir?: string | null;
}) {
  const t = useT();
  const repositoryOptions = React.useMemo(
    () =>
      projects.flatMap((project) =>
        project.repositories.map((repository) => ({ project, repository })),
      ),
    [projects],
  );
  const initialProject =
    projects.find((project) => project.id === initialProjectId) ?? projects[0];
  const initialRepository = selectProjectRepository(initialProject, null);
  const [repositoryId, setRepositoryId] = React.useState(
    initialRepository?.id ?? "",
  );
  const selection =
    repositoryOptions.find(
      (candidate) => candidate.repository.id === repositoryId,
    ) ?? repositoryOptions[0];
  const project = selection?.project;
  const repository = selection?.repository;
  const repoStateQuery = useRepoStateQuery(repository);
  const pullRequestsQuery = useProjectPullRequestsQuery(repository);
  const initialSyncQuery = useProjectRepoSyncStatusQuery(
    repository,
    reposDir,
    repository?.defaultBranch,
  );
  const branchOptions = React.useMemo(() => {
    const names = [
      repository?.defaultBranch,
      ...(repoStateQuery.data?.branches.map((branch) => branch.name) ?? []),
      initialSyncQuery.data?.localBranch,
    ].filter((name): name is string => Boolean(name));
    return [...new Set(names)];
  }, [
    initialSyncQuery.data?.localBranch,
    repository?.defaultBranch,
    repoStateQuery.data?.branches,
  ]);
  const [targetBranch, setTargetBranch] = React.useState(
    repository?.defaultBranch ?? "",
  );
  const [sourceBranch, setSourceBranch] = React.useState("");
  const sourceSyncQuery = useProjectRepoSyncStatusQuery(
    repository,
    reposDir,
    sourceBranch || null,
    targetBranch || null,
  );
  const createMutation = useCreateProjectPullRequestMutation(repository);

  React.useEffect(() => {
    if (!open) return;
    const nextProject =
      projects.find((candidate) => candidate.id === initialProjectId) ??
      projects[0];
    setRepositoryId(selectProjectRepository(nextProject, null)?.id ?? "");
  }, [initialProjectId, open, projects]);

  React.useEffect(() => {
    if (!repository) return;
    setTargetBranch(repository.defaultBranch);
    setSourceBranch("");
  }, [repository]);

  React.useEffect(() => {
    if (
      sourceBranch &&
      branchOptions.includes(sourceBranch) &&
      sourceBranch !== targetBranch
    ) {
      return;
    }
    setSourceBranch(
      branchOptions.find((branch) => branch !== targetBranch) ?? "",
    );
  }, [branchOptions, sourceBranch, targetBranch]);

  const sourceCommit =
    repoStateQuery.data?.branches.find((branch) => branch.name === sourceBranch)
      ?.commit ??
    (sourceSyncQuery.data?.remoteBranch === sourceBranch
      ? sourceSyncQuery.data.remoteHead
      : null);
  const hasOpenPullRequest = (pullRequestsQuery.data ?? []).some(
    (pullRequest) =>
      (pullRequest.status === "Open" || pullRequest.status === "Draft") &&
      pullRequest.branchName === sourceBranch &&
      (pullRequest.targetBranch ?? repository?.defaultBranch) === targetBranch,
  );
  const selectionError = !repository
    ? t("projects.createTask.chooseRepository")
    : !targetBranch
      ? t("projects.createReview.chooseBase")
      : !sourceBranch
        ? t("projects.createReview.chooseCompare")
        : sourceBranch === targetBranch
          ? t("projects.createReview.sameBranches")
          : hasOpenPullRequest
            ? t("projects.createReview.alreadyOpen")
            : !sourceCommit
              ? t("projects.createReview.notPushed")
              : null;
  const description =
    repository && sourceBranch && targetBranch
      ? sourceCommit
        ? t("projects.createReview.compareAtCommit", {
            repository: repository.name,
            source: sourceBranch,
            target: targetBranch,
            commit: sourceCommit.slice(0, 7),
          })
        : t("projects.createReview.compare", {
            repository: repository.name,
            source: sourceBranch,
            target: targetBranch,
          })
      : t("projects.createReview.chooseBranches");

  async function handleCreate(input: CreatePullRequestDialogInput) {
    if (!project || !repository || !sourceCommit || selectionError) {
      throw new Error(selectionError ?? t("projects.createReview.incomplete"));
    }
    const pullRequestId = await createMutation.mutateAsync({
      ...input,
      branch: sourceBranch,
      targetBranch,
      commit: sourceCommit,
      mergeBase: sourceSyncQuery.data?.mergeBase ?? null,
      reviewers: [],
    });
    toast.success(t("projects.createReview.created"));
    await onCreated(project, repository, pullRequestId);
  }

  return (
    <CreateProjectWorkItemDialog
      bodyPlaceholder={t("projects.createReview.bodyPlaceholder")}
      description={description}
      isCreating={createMutation.isPending}
      itemName="pull-request"
      onCreate={handleCreate}
      onOpenChange={(nextOpen) => {
        if (!nextOpen && createMutation.isPending) return;
        onOpenChange(nextOpen);
      }}
      open={open}
      submitDisabled={Boolean(selectionError)}
      title={t("projects.createReview.title")}
      titlePlaceholder={t("projects.createReview.titlePlaceholder")}
    >
      <div className="grid gap-3 rounded-xl border border-border/60 bg-muted/25 p-3 sm:grid-cols-2">
        <label className="space-y-1.5 text-sm font-medium sm:col-span-2">
          <span>{t("projects.createTask.repository")}</span>
          <select
            className="h-10 w-full rounded-lg border border-input bg-background px-3 text-sm font-normal outline-hidden focus:ring-1 focus:ring-ring"
            data-testid="create-pull-request-repository"
            disabled={createMutation.isPending}
            onChange={(event) => setRepositoryId(event.target.value)}
            value={repository?.id ?? ""}
          >
            {repositoryOptions.map((candidate) => (
              <option
                key={candidate.repository.id}
                value={candidate.repository.id}
              >
                {candidate.project.repositories.length > 1
                  ? `${candidate.project.name} / ${candidate.repository.name}`
                  : candidate.project.name}
              </option>
            ))}
          </select>
        </label>
        <label className="space-y-1.5 text-sm font-medium">
          <span>{t("projects.createReview.base")}</span>
          <select
            className="h-10 w-full rounded-lg border border-input bg-background px-3 text-sm font-normal outline-hidden focus:ring-1 focus:ring-ring"
            data-testid="create-pull-request-base-branch"
            disabled={createMutation.isPending}
            onChange={(event) => setTargetBranch(event.target.value)}
            value={targetBranch}
          >
            {branchOptions.map((branch) => (
              <option key={branch} value={branch}>
                {branch}
              </option>
            ))}
          </select>
        </label>
        <label className="space-y-1.5 text-sm font-medium">
          <span>{t("projects.createReview.compareLabel")}</span>
          <select
            className="h-10 w-full rounded-lg border border-input bg-background px-3 text-sm font-normal outline-hidden focus:ring-1 focus:ring-ring"
            data-testid="create-pull-request-compare-branch"
            disabled={createMutation.isPending}
            onChange={(event) => setSourceBranch(event.target.value)}
            value={sourceBranch}
          >
            <option disabled value="">
              {t("projects.createReview.selectBranch")}
            </option>
            {branchOptions.map((branch) => (
              <option key={branch} value={branch}>
                {branch}
              </option>
            ))}
          </select>
        </label>
        {selectionError ? (
          <p className="text-xs text-muted-foreground sm:col-span-2">
            {selectionError}
          </p>
        ) : null}
      </div>
    </CreateProjectWorkItemDialog>
  );
}
