import type {
  ProjectRepoSnapshot,
  Repository as Project,
} from "@/features/projects/hooks";
import type { EntityLinkTab } from "@/shared/lib/entityLink";
import { normalizePubkey } from "@/shared/lib/pubkey";
import { type MessageKey, t } from "@/shared/i18n";

export const PROJECT_REPOSITORY_SEARCH_KEYS = [
  "repositoryId",
  "issueId",
  "pullRequestId",
  "commitHash",
  "filePath",
] as const;

/** Message keys; translate where shown. */
export const PROJECT_TAB_CRUMB_LABELS: Record<string, MessageKey> = {
  files: "projects.crumbs.files",
  activity: "projects.crumbs.commits",
  issues: "projects.crumbs.tasks",
  prs: "projects.crumbs.review",
  contributors: "projects.crumbs.contributors",
  channels: "projects.crumbs.groups",
};

export type ProjectDetailScreenProps = {
  commitHash?: string;
  entityNavigationId?: string;
  filePath?: string;
  projectId: string;
  pullRequestId?: string;
  issueId?: string;
  repositoryId?: string;
  /** Workspace tab requested by a share link (link vocabulary). */
  tab?: EntityLinkTab;
};

/** Tooltip for the push/pull sync buttons, e.g. "Pull 2 remote commits". */
export function pushPullTitle(
  verb: "Push" | "Pull",
  count: number | undefined,
  side: "local" | "remote",
) {
  const key: MessageKey =
    verb === "Push"
      ? side === "local"
        ? "projects.sync.pushLocal"
        : "projects.sync.pushRemote"
      : side === "local"
        ? "projects.sync.pullLocal"
        : "projects.sync.pullRemote";
  return t(key, { count: count ?? 0 });
}

/** Returns the normalized owner and contributor pubkeys for a project. */
export function projectPeople(project: Project) {
  return [
    ...new Set(
      [project.owner, ...project.contributors]
        .filter(Boolean)
        .map(normalizePubkey),
    ),
  ];
}

/** Reports whether a repository snapshot contains any displayable content. */
export function snapshotHasContent(
  snapshot: ProjectRepoSnapshot | null | undefined,
) {
  return Boolean(
    snapshot &&
      (snapshot.latestCommit ||
        snapshot.commits.length > 0 ||
        snapshot.files.length > 0 ||
        snapshot.contributors.length > 0),
  );
}
