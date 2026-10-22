import { useMutation, useQueryClient } from "@tanstack/react-query";
import * as React from "react";

import {
  signProjectPullRequestReviewRequest,
  signProjectPullRequestStatus,
} from "@/shared/api/projectGit";
import { relayClient } from "@/shared/api/relayClient";
import { signRelayEvent } from "@/shared/api/tauri";
import { normalizePubkey } from "@/shared/lib/pubkey";
import {
  KIND_GIT_STATUS_CLOSED,
  KIND_GIT_STATUS_DRAFT,
  KIND_GIT_STATUS_OPEN,
  KIND_TEXT_NOTE,
} from "@/shared/constants/kinds";
import type { Repository as Project } from "./hooks";
import {
  nextProjectPullRequestStatusCreatedAt,
  type ProjectPullRequest,
} from "./projectPullRequests.mjs";
import {
  PR_APPROVAL_LABEL,
  PR_CHANGES_REQUESTED_LABEL,
  PR_REVIEW_REQUEST_LABEL,
} from "./projectPullRequests.mjs";
import { type MessageKey, t } from "@/shared/i18n";

/** NIP-34 lifecycle states the desktop can publish for a PR. Merged (1631)
 * is intentionally excluded — merges happen through git, not this UI. */
export type ProjectPullRequestLifecycleStatus = "open" | "draft" | "closed";

const PR_STATUS_KIND_BY_LIFECYCLE: Record<
  ProjectPullRequestLifecycleStatus,
  number
> = {
  open: KIND_GIT_STATUS_OPEN,
  draft: KIND_GIT_STATUS_DRAFT,
  closed: KIND_GIT_STATUS_CLOSED,
};

// Same shape as `kuzz pr status` (kuzz-sdk build_git_status): root `e` tag,
// repo `a` tag, and `p` tags for the repo owner + PR author. Only the PR
// author or repo owner are trusted for status changes (allowedActorsForRoot).
async function updateProjectPullRequestStatus({
  project,
  pullRequest,
  signAsManagedOwner,
  status,
}: {
  project: Project;
  pullRequest: ProjectPullRequest;
  signAsManagedOwner: boolean;
  status: ProjectPullRequestLifecycleStatus;
}): Promise<void> {
  const createdAt = nextProjectPullRequestStatusCreatedAt(
    pullRequest,
    Math.floor(Date.now() / 1_000),
  );
  if (signAsManagedOwner) {
    await signProjectPullRequestStatus({
      targetOwner: project.owner,
      repoAddress: project.repoAddress,
      pullRequestId: pullRequest.id,
      pullRequestAuthor: pullRequest.author,
      status,
      createdAt,
    });
    return;
  }
  const recipients = new Set([
    project.owner.toLowerCase(),
    pullRequest.author.toLowerCase(),
  ]);
  const event = await signRelayEvent({
    kind: PR_STATUS_KIND_BY_LIFECYCLE[status],
    content: "",
    createdAt,
    tags: [
      ["e", pullRequest.id, "", "root"],
      ["a", project.repoAddress],
      ...[...recipients].map((recipient) => ["p", recipient]),
    ],
  });

  await relayClient.publishEvent(
    event,
    t("projects.pullRequestReviews.statusTimedOut"),
    t("projects.pullRequestReviews.statusFailed"),
  );
}

// Review requests and approvals are labeled kind:1 comments (see
// projectPullRequests.mjs) — NIP-34 has no dedicated review kinds, and the
// relay does not register kind 1111. `p` tags on a review request are the
// requested reviewers; parsing only trusts requests signed by the PR author
// or repo owner.
async function requestProjectPullRequestReview({
  project,
  pullRequest,
  reviewers,
  reviewerLabel,
  signAsManagedOwner,
}: {
  project: Project;
  pullRequest: ProjectPullRequest;
  reviewers: string[];
  reviewerLabel: string;
  signAsManagedOwner: boolean;
}): Promise<void> {
  if (reviewers.length === 0) {
    throw new Error(t("projects.pullRequestReviews.selectReviewer"));
  }
  const reviewerPubkeys = [
    ...new Set(reviewers.map((pubkey) => pubkey.toLowerCase())),
  ];
  if (signAsManagedOwner) {
    await signProjectPullRequestReviewRequest({
      targetOwner: project.owner,
      repoAddress: project.repoAddress,
      pullRequestId: pullRequest.id,
      reviewers: reviewerPubkeys,
      reviewerLabel,
    });
    return;
  }
  const event = await signRelayEvent({
    kind: KIND_TEXT_NOTE,
    content: `Requested a review from ${reviewerLabel}`,
    tags: [
      ["e", pullRequest.id, "", "root"],
      ["a", project.repoAddress],
      ...reviewerPubkeys.map((pubkey) => ["p", pubkey]),
      ["t", PR_REVIEW_REQUEST_LABEL],
    ],
  });

  await relayClient.publishEvent(
    event,
    t("projects.pullRequestReviews.requestTimedOut"),
    t("projects.pullRequestReviews.requestFailed"),
  );
}

type ProjectPullRequestReviewDecision = "approve" | "request-changes";

/** Whether a viewer may submit a review decision for this pull request. */
export function canReviewProjectPullRequest(
  project: Project,
  pullRequest: ProjectPullRequest,
  viewerPubkey: string | null | undefined,
) {
  if (
    !viewerPubkey ||
    !pullRequest.commit ||
    (pullRequest.status !== "Open" && pullRequest.status !== "Draft")
  ) {
    return false;
  }
  const viewer = normalizePubkey(viewerPubkey);
  if (viewer === normalizePubkey(pullRequest.author)) return false;
  return (
    viewer === normalizePubkey(project.owner) ||
    pullRequest.reviewers.some(
      (reviewer) => normalizePubkey(reviewer) === viewer,
    )
  );
}

const REVIEW_DECISION_DETAILS: Record<
  ProjectPullRequestReviewDecision,
  {
    content: string;
    errorMessage: MessageKey;
    label: string;
    timeoutMessage: MessageKey;
  }
> = {
  approve: {
    content: "Approved these changes",
    errorMessage: "projects.pullRequestReviews.approveFailed",
    label: PR_APPROVAL_LABEL,
    timeoutMessage: "projects.pullRequestReviews.approveTimedOut",
  },
  "request-changes": {
    content: "Requested changes",
    errorMessage: "projects.pullRequestReviews.requestChangesFailed",
    label: PR_CHANGES_REQUESTED_LABEL,
    timeoutMessage: "projects.pullRequestReviews.requestChangesTimedOut",
  },
};

async function submitProjectPullRequestReview({
  content,
  createdAt,
  decision,
  project,
  pullRequest,
}: {
  content?: string;
  createdAt: number;
  decision: ProjectPullRequestReviewDecision;
  project: Project;
  pullRequest: ProjectPullRequest;
}): Promise<void> {
  if (!pullRequest.commit) {
    throw new Error(t("projects.pullRequestReviews.noCommit"));
  }
  const details = REVIEW_DECISION_DETAILS[decision];
  const recipients = new Set([
    project.owner.toLowerCase(),
    pullRequest.author.toLowerCase(),
  ]);
  const event = await signRelayEvent({
    kind: KIND_TEXT_NOTE,
    content: content?.trim() || details.content,
    createdAt,
    tags: [
      ["e", pullRequest.id, "", "root"],
      ["a", project.repoAddress],
      ...[...recipients].map((recipient) => ["p", recipient]),
      ["t", details.label],
      ["c", pullRequest.commit],
    ],
  });

  await relayClient.publishEvent(
    event,
    t(details.timeoutMessage),
    t(details.errorMessage),
  );
}

export function useProjectPullRequestWriteInvalidation(
  project: Project | null | undefined,
) {
  const queryClient = useQueryClient();
  return React.useCallback(() => {
    void queryClient.invalidateQueries({
      queryKey: ["project", project?.id ?? "none", "pull-requests"],
    });
    void queryClient.invalidateQueries({
      queryKey: ["projects", "work-items"],
    });
    void queryClient.invalidateQueries({
      queryKey: ["projects", "activity-summaries"],
    });
  }, [project?.id, queryClient]);
}

export function useUpdateProjectPullRequestStatusMutation(
  project: Project | null | undefined,
) {
  const invalidate = useProjectPullRequestWriteInvalidation(project);

  return useMutation({
    mutationFn: ({
      pullRequest,
      signAsManagedOwner = false,
      status,
    }: {
      pullRequest: ProjectPullRequest;
      signAsManagedOwner?: boolean;
      status: ProjectPullRequestLifecycleStatus;
    }) => {
      if (!project) throw new Error(t("projects.errors.noProjectSelected"));
      return updateProjectPullRequestStatus({
        project,
        pullRequest,
        signAsManagedOwner,
        status,
      });
    },
    onSuccess: invalidate,
  });
}

export function useRequestProjectPullRequestReviewMutation(
  project: Project | null | undefined,
) {
  const invalidate = useProjectPullRequestWriteInvalidation(project);

  return useMutation({
    mutationFn: ({
      pullRequest,
      reviewers,
      reviewerLabel,
      signAsManagedOwner,
    }: {
      pullRequest: ProjectPullRequest;
      reviewers: string[];
      reviewerLabel: string;
      signAsManagedOwner: boolean;
    }) => {
      if (!project) throw new Error(t("projects.errors.noProjectSelected"));
      return requestProjectPullRequestReview({
        project,
        pullRequest,
        reviewers,
        reviewerLabel,
        signAsManagedOwner,
      });
    },
    onSuccess: invalidate,
  });
}

function useProjectPullRequestReviewDecisionMutation(
  project: Project | null | undefined,
  decision: ProjectPullRequestReviewDecision,
) {
  const invalidate = useProjectPullRequestWriteInvalidation(project);

  return useMutation({
    mutationFn: ({
      content,
      createdAt,
      pullRequest,
    }: {
      content?: string;
      createdAt: number;
      pullRequest: ProjectPullRequest;
    }) => {
      if (!project) throw new Error(t("projects.errors.noProjectSelected"));
      return submitProjectPullRequestReview({
        content,
        createdAt,
        decision,
        project,
        pullRequest,
      });
    },
    onSuccess: invalidate,
  });
}

export function useApproveProjectPullRequestMutation(
  project: Project | null | undefined,
) {
  return useProjectPullRequestReviewDecisionMutation(project, "approve");
}

export function useRequestProjectPullRequestChangesMutation(
  project: Project | null | undefined,
) {
  return useProjectPullRequestReviewDecisionMutation(
    project,
    "request-changes",
  );
}
