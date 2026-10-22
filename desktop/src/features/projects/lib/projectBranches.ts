import { t } from "@/shared/i18n";
const BRANCH_CHARACTERS = /^[A-Za-z0-9/_.-]+$/;

/** Normalize a branch name using the native command's conservative rules. */
export function normalizeProjectBranchName(value: string): string | null {
  const trimmed = value.trim();
  if (trimmed.startsWith("refs/") && !trimmed.startsWith("refs/heads/")) {
    return null;
  }
  const branch = trimmed.replace(/^refs\/heads\//, "");
  if (
    !branch ||
    branch.startsWith("-") ||
    branch.startsWith("/") ||
    branch.endsWith("/") ||
    branch.endsWith(".") ||
    branch.endsWith(".lock") ||
    branch.includes("..") ||
    branch.includes("//") ||
    !BRANCH_CHARACTERS.test(branch) ||
    branch.split("/").some((component) => component.startsWith("."))
  ) {
    return null;
  }
  return branch;
}

export function projectBranchNameError(
  value: string,
  existingBranches: string[],
): string | null {
  const branch = normalizeProjectBranchName(value);
  if (!branch) return t("projects.branches.invalidName");
  if (existingBranches.includes(branch)) {
    return t("projects.branches.nameTaken");
  }
  return null;
}

export function projectBranchOptions(
  remoteBranches: string[],
  localBranches: string[] = [],
): string[] {
  return [...new Set([...remoteBranches, ...localBranches].filter(Boolean))];
}

export function projectBranchOptionsFromSync(
  remoteBranches: string[],
  syncStatus?: {
    localBranch: string | null;
    localBranches: string[];
    localHead: string | null;
  },
): string[] {
  const localBranches =
    syncStatus?.localBranches ??
    (syncStatus?.localHead && syncStatus.localBranch
      ? [syncStatus.localBranch]
      : []);
  return projectBranchOptions(remoteBranches, localBranches);
}

export function projectBranchCreationReason(input: {
  activeBranch: string | null;
  activeBranchCommit: string | null;
  localHead?: string | null;
}): string | null {
  if (!input.activeBranch) return t("projects.branches.chooseFirst");
  if (input.activeBranchCommit) return null;
  return input.localHead
    ? t("projects.branches.pushFirstCommit", { branch: input.activeBranch })
    : t("projects.branches.createFirstCommit");
}

/** Resolve a usable default branch when a repository advertises a stale HEAD. */
export function resolveProjectDefaultBranch(
  announcedBranch: string,
  repoState?: {
    branches: Array<{ name: string }>;
    head: string | null;
  } | null,
): string {
  if (!repoState || repoState.branches.length === 0) {
    return repoState?.head ?? announcedBranch;
  }
  const published = new Set(repoState.branches.map((branch) => branch.name));
  if (repoState.head && published.has(repoState.head)) return repoState.head;
  if (published.has(announcedBranch)) return announcedBranch;
  if (published.has("main")) return "main";
  if (published.has("master")) return "master";
  return repoState.branches[0]?.name ?? announcedBranch;
}

export function projectBranchManagementState(input: {
  activeBranch: string | null;
  defaultBranch: string | null;
  branches: Array<{ name: string; commit: string }>;
  remoteBranch?: string | null;
  remoteHead?: string | null;
  snapshotCommit?: string | null;
  hasOpenPullRequest: boolean;
}) {
  const activeRemoteBranch =
    input.branches.find((branch) => branch.name === input.activeBranch) ?? null;
  const activeBranchCommit =
    activeRemoteBranch?.commit ??
    (input.remoteBranch === input.activeBranch ? input.remoteHead : null) ??
    input.snapshotCommit ??
    null;
  const deleteBranchReason = !input.activeBranch
    ? t("projects.branches.chooseFirst")
    : input.activeBranch === input.defaultBranch
      ? t("projects.branches.defaultNotDeletable")
      : !activeRemoteBranch
        ? t("projects.branches.onlyRemoteDeletable")
        : input.hasOpenPullRequest
          ? t("projects.branches.closeReviewFirst")
          : null;
  return { activeBranchCommit, activeRemoteBranch, deleteBranchReason };
}
