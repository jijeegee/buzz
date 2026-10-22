import type { ProjectIssue } from "@/features/projects/projectIssues.mjs";
import type { ProjectPullRequest } from "@/features/projects/projectPullRequests.mjs";
import { type MessageKey, t } from "@/shared/i18n";

// Status values are protocol strings stored on relay events; only the
// on-screen label is translated.
const ISSUE_STATUS_KEYS: Record<string, MessageKey> = {
  Triage: "projects.status.triage",
  Backlog: "projects.status.backlog",
  "In Progress": "projects.status.inProgress",
  "In Review": "projects.status.inReview",
  Done: "projects.status.done",
  Closed: "projects.status.closed",
};

const PULL_REQUEST_STATUS_KEYS: Record<string, MessageKey> = {
  Open: "projects.status.open",
  Merged: "projects.status.merged",
  Closed: "projects.status.closed",
  Draft: "projects.status.draft",
};

/** On-screen label for a task status such as "In Review". */
export function issueStatusLabel(status: ProjectIssue["status"]): string {
  const key = ISSUE_STATUS_KEYS[status];
  return key ? t(key) : status;
}

/** On-screen label for a review (pull request) status such as "Merged". */
export function pullRequestStatusLabel(
  status: ProjectPullRequest["status"],
): string {
  const key = PULL_REQUEST_STATUS_KEYS[status];
  return key ? t(key) : status;
}
