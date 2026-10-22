import type { Project } from "@/features/projects/hooks";
import { t } from "@/shared/i18n";

export function getDiscussionLabel(project: Project) {
  return project.projectChannelId
    ? t("projects.labels.discussionLinked")
    : t("projects.labels.noDiscussion");
}
