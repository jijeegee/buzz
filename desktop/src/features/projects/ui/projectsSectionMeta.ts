import {
  CircleDot,
  FolderGit2,
  Folders,
  GitPullRequest,
  type LucideIcon,
  Users,
} from "lucide-react";

import type { ProjectsFilter } from "@/features/projects/lib/projectsViewHelpers";
import { t } from "@/shared/i18n";

export function projectsSectionTitle(filter: ProjectsFilter) {
  if (filter === "all") return t("projects.sectionTitle.activity");
  if (filter === "prs") return t("projects.sectionTitle.reviews");
  if (filter === "issues") return t("projects.sectionTitle.tasks");
  if (filter === "repositories") return t("projects.sectionTitle.repositories");
  if (filter === "channels") return t("projects.sectionTitle.groups");
  return t("projects.sectionTitle.projects");
}

/** Placeholder of the Projects section search field, e.g. "Search tasks". */
export function projectsSectionSearchPlaceholder(filter: ProjectsFilter) {
  if (filter === "all") return t("projects.sectionSearch.placeholderActivity");
  if (filter === "prs") return t("projects.sectionSearch.placeholderReviews");
  if (filter === "issues") return t("projects.sectionSearch.placeholderTasks");
  if (filter === "repositories") {
    return t("projects.sectionSearch.placeholderRepositories");
  }
  if (filter === "channels")
    return t("projects.sectionSearch.placeholderGroups");
  return t("projects.sectionSearch.placeholderProjects");
}

export function projectsSectionIcon(filter: ProjectsFilter): LucideIcon {
  if (filter === "prs") return GitPullRequest;
  if (filter === "issues") return CircleDot;
  if (filter === "repositories") return FolderGit2;
  if (filter === "channels") return Users;
  return Folders;
}

export function openAppSearch() {
  document
    .querySelector<HTMLButtonElement>('[data-testid="open-search"]')
    ?.click();
}
