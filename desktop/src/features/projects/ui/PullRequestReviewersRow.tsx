import {
  Check,
  History,
  Search,
  TriangleAlert,
  UserPlus,
  Users,
} from "lucide-react";
import * as React from "react";
import { toast } from "sonner";

import { useIsArchivedPredicate } from "@/features/identity-archive/hooks";
import type {
  ProjectPullRequest,
  Repository as Project,
} from "@/features/projects/hooks";
import { useRequestProjectPullRequestReviewMutation } from "@/features/projects/pullRequestReviews";
import { useUserSearchQuery } from "@/features/profile/hooks";
import type { UserProfileLookup } from "@/features/profile/lib/identity";
import type { UserSearchResult } from "@/shared/api/types";
import { cn } from "@/shared/lib/cn";
import { normalizePubkey, UNKNOWN_USER_LABEL } from "@/shared/lib/pubkey";
import { Button } from "@/shared/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/shared/ui/dialog";
import { Input } from "@/shared/ui/input";
import { UserAvatar } from "@/shared/ui/UserAvatar";

import { ProjectDetailMetaRow } from "./ProjectDetailMeta";
import { PROJECT_CONTEXT_ACTION_BUTTON_CLASS } from "./projectContextActionStyles";
import { useT } from "@/shared/i18n";

function profileForPubkey(pubkey: string, profiles?: UserProfileLookup) {
  return profiles?.[normalizePubkey(pubkey)] ?? null;
}

function labelForPubkey(pubkey: string, profiles?: UserProfileLookup) {
  const profile = profileForPubkey(pubkey, profiles);
  return (
    profile?.displayName?.trim() ||
    profile?.nip05Handle?.trim() ||
    UNKNOWN_USER_LABEL
  );
}

function reviewerSearchLabel(user: UserSearchResult) {
  return (
    user.displayName?.trim() || user.nip05Handle?.trim() || UNKNOWN_USER_LABEL
  );
}

/** Reviewer status avatars and the reviewer request picker for a pull request. */
export function PullRequestReviewersRow({
  actionLabel,
  canRequest,
  contextActions = false,
  profiles,
  project,
  pullRequest,
  signAsManagedOwner,
  showDecisionActors = true,
  showSummary = true,
  summaryTestId = "project-review-summary",
}: {
  actionLabel?: string;
  canRequest: boolean;
  contextActions?: boolean;
  profiles?: UserProfileLookup;
  project: Project;
  pullRequest: ProjectPullRequest;
  signAsManagedOwner: boolean;
  showDecisionActors?: boolean;
  showSummary?: boolean;
  summaryTestId?: string;
}) {
  const t = useT();
  const [pickerOpen, setPickerOpen] = React.useState(false);
  const [reviewerQuery, setReviewerQuery] = React.useState("");
  const requestInFlightRef = React.useRef(false);
  const requestReviewMutation =
    useRequestProjectPullRequestReviewMutation(project);
  const deferredReviewerQuery = React.useDeferredValue(reviewerQuery.trim());
  const requestedReviewers = React.useMemo(
    () => new Set(pullRequest.reviewers.map(normalizePubkey)),
    [pullRequest.reviewers],
  );
  const pullRequestAuthor = normalizePubkey(pullRequest.author);
  const userSearchQuery = useUserSearchQuery(deferredReviewerQuery, {
    allowEmpty: true,
    enabled: canRequest && pickerOpen,
    limit: 50,
  });
  const isArchivedDiscovery = useIsArchivedPredicate();
  const candidates = React.useMemo(
    () =>
      (userSearchQuery.data ?? []).filter((user) => {
        const pubkey = normalizePubkey(user.pubkey);
        return (
          pubkey !== pullRequestAuthor &&
          !requestedReviewers.has(pubkey) &&
          !isArchivedDiscovery(pubkey)
        );
      }),
    [
      isArchivedDiscovery,
      pullRequestAuthor,
      requestedReviewers,
      userSearchQuery.data,
    ],
  );
  const approvedBy = new Set(
    pullRequest.approvals.map((approval) => normalizePubkey(approval.author)),
  );
  const changesRequestedBy = new Set(
    pullRequest.changeRequests.map((request) =>
      normalizePubkey(request.author),
    ),
  );
  const historicalBy = new Set(
    pullRequest.comments
      .filter(
        (comment) =>
          comment.isTrustedReviewDecision &&
          comment.reviewDecisionStatus === "historical",
      )
      .map((comment) => normalizePubkey(comment.author)),
  );
  const decisionActors = [
    ...new Set([
      ...pullRequest.reviewers.map(normalizePubkey),
      ...pullRequest.approvals.map((approval) =>
        normalizePubkey(approval.author),
      ),
      ...pullRequest.changeRequests.map((request) =>
        normalizePubkey(request.author),
      ),
      ...historicalBy,
    ]),
  ];
  const requestedApprovalCount = pullRequest.reviewers.filter((pubkey) =>
    approvedBy.has(normalizePubkey(pubkey)),
  ).length;
  const staleDecisionActors = new Set(
    pullRequest.commit
      ? [...historicalBy].filter(
          (pubkey) =>
            !approvedBy.has(pubkey) && !changesRequestedBy.has(pubkey),
        )
      : [],
  );
  const hasHistoricalDecision = staleDecisionActors.size > 0;
  const reviewSummary = !pullRequest.commit
    ? t("projects.reviewers.noCommit")
    : changesRequestedBy.size > 0
      ? t("projects.activity.changesRequested")
      : pullRequest.reviewers.length > 0 &&
          requestedApprovalCount === pullRequest.reviewers.length
        ? t("projects.activity.approved")
        : pullRequest.reviewers.length === 0 && approvedBy.size > 0
          ? t("projects.activity.approved")
          : hasHistoricalDecision &&
              approvedBy.size === 0 &&
              changesRequestedBy.size === 0
            ? t("projects.reviewers.rereviewNeeded")
            : requestedApprovalCount > 0
              ? t("projects.reviewers.approvedCount", {
                  approved: requestedApprovalCount,
                  total: pullRequest.reviewers.length,
                })
              : pullRequest.reviewers.length > 0
                ? t("projects.reviewers.awaiting")
                : t("projects.reviewers.none");

  const handleRequest = React.useCallback(
    async (pubkey: string, reviewerLabel: string) => {
      if (requestReviewMutation.isPending || requestInFlightRef.current) return;
      requestInFlightRef.current = true;
      try {
        await requestReviewMutation.mutateAsync({
          pullRequest,
          reviewers: [pubkey],
          reviewerLabel,
          signAsManagedOwner,
        });
        setPickerOpen(false);
        setReviewerQuery("");
        toast.success(t("projects.reviewers.requested"));
      } catch (error) {
        toast.error(
          error instanceof Error
            ? error.message
            : t("projects.reviewers.requestFailed"),
        );
      } finally {
        requestInFlightRef.current = false;
      }
    },
    [pullRequest, requestReviewMutation, signAsManagedOwner, t],
  );

  React.useEffect(() => {
    if (!pickerOpen) setReviewerQuery("");
  }, [pickerOpen]);
  const displayedDecisionActors = showDecisionActors ? decisionActors : [];
  const requestAction = canRequest ? (
    <Dialog onOpenChange={setPickerOpen} open={pickerOpen}>
      <DialogTrigger asChild>
        <Button
          className={cn(
            "h-5 px-0 text-sm text-muted-foreground hover:bg-transparent hover:text-foreground",
            contextActions && PROJECT_CONTEXT_ACTION_BUTTON_CLASS,
          )}
          disabled={requestReviewMutation.isPending}
          size="xs"
          type="button"
          variant="ghost"
        >
          {contextActions ? <UserPlus /> : null}
          {actionLabel ?? t("projects.reviewers.addReviewer")}
        </Button>
      </DialogTrigger>
      <DialogContent className="max-w-md gap-0 overflow-hidden p-0">
        <DialogHeader className="border-b border-border/60 px-6 py-5 pr-14">
          <DialogTitle>{t("projects.reviewers.dialogTitle")}</DialogTitle>
          <DialogDescription>
            {t("projects.reviewers.dialogDescription")}
          </DialogDescription>
        </DialogHeader>
        <div className="flex items-center gap-2 border-b border-border/60 px-6 py-3">
          <Search className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
          <Input
            autoFocus
            className="h-8 border-0 px-0 text-sm shadow-none focus-visible:ring-0"
            data-testid="project-reviewer-search"
            onChange={(event) => setReviewerQuery(event.target.value)}
            placeholder={t("projects.reviewers.searchPlaceholder")}
            value={reviewerQuery}
          />
        </div>
        <div className="max-h-72 min-h-28 overflow-y-auto p-2">
          {userSearchQuery.isLoading ? (
            <p className="px-3 py-4 text-sm text-muted-foreground">
              {t("projects.reviewers.searching")}
            </p>
          ) : candidates.length > 0 ? (
            candidates.map((candidate) => {
              const label = reviewerSearchLabel(candidate);
              return (
                <button
                  className="flex w-full min-w-0 items-center gap-3 rounded-lg px-3 py-2 text-left hover:bg-accent disabled:cursor-not-allowed disabled:opacity-50"
                  data-testid={`project-reviewer-result-${candidate.pubkey}`}
                  disabled={requestReviewMutation.isPending}
                  key={candidate.pubkey}
                  onClick={() => {
                    void handleRequest(candidate.pubkey, label);
                  }}
                  type="button"
                >
                  <UserAvatar
                    accent={candidate.isAgent}
                    avatarUrl={candidate.avatarUrl}
                    displayName={label}
                    shape={candidate.isAgent ? "squircle" : "circle"}
                    size="xs"
                  />
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-sm font-medium text-foreground">
                      {label}
                    </span>
                    {candidate.isAgent ? (
                      <span className="block truncate text-xs text-muted-foreground">
                        {t("projects.assignees.agent")}
                      </span>
                    ) : null}
                  </span>
                </button>
              );
            })
          ) : (
            <p className="px-3 py-4 text-sm text-muted-foreground">
              {t("projects.reviewers.noMatches")}
            </p>
          )}
        </div>
      </DialogContent>
    </Dialog>
  ) : null;

  if (contextActions) return requestAction;

  return (
    <ProjectDetailMetaRow icon={Users} label={t("projects.reviewers.label")}>
      <div
        className="flex min-w-0 items-center gap-2"
        data-testid="project-reviewers-content"
      >
        <div className="flex min-w-0 flex-1 items-center gap-2 overflow-hidden whitespace-nowrap">
          {showSummary && displayedDecisionActors.length === 0 ? (
            <span className="truncate font-medium" data-testid={summaryTestId}>
              {reviewSummary}
            </span>
          ) : null}
          {displayedDecisionActors.map((pubkey, index) => {
            const label = labelForPubkey(pubkey, profiles);
            const hasApproved = approvedBy.has(pubkey);
            const hasRequestedChanges = changesRequestedBy.has(pubkey);
            const needsRereview = staleDecisionActors.has(pubkey);
            const DecisionIcon = hasApproved
              ? Check
              : hasRequestedChanges
                ? TriangleAlert
                : needsRereview
                  ? History
                  : null;
            const decisionLabel = hasApproved
              ? t("projects.reviewers.approvedBy", { name: label })
              : hasRequestedChanges
                ? t("projects.reviewers.changesRequestedBy", { name: label })
                : needsRereview
                  ? t("projects.reviewers.rereviewFrom", { name: label })
                  : t("projects.reviewers.awaitingFrom", { name: label });
            return (
              <React.Fragment key={pubkey}>
                {index > 0 ? (
                  <span
                    aria-hidden="true"
                    className="shrink-0 text-muted-foreground/50"
                  >
                    ·
                  </span>
                ) : null}
                <span
                  className={cn(
                    "flex min-w-0 shrink items-center gap-1 text-sm",
                    hasApproved && "text-green-600 dark:text-green-400",
                    hasRequestedChanges && "text-amber-600 dark:text-amber-400",
                    !hasApproved &&
                      !hasRequestedChanges &&
                      "text-muted-foreground",
                  )}
                  data-testid="project-reviewer-decision"
                  title={decisionLabel}
                >
                  <span className="sr-only">{decisionLabel}</span>
                  {DecisionIcon ? (
                    <DecisionIcon
                      aria-hidden="true"
                      className="h-3.5 w-3.5 shrink-0"
                    />
                  ) : null}
                  <span
                    aria-hidden="true"
                    className="truncate"
                    data-testid="project-reviewer-name"
                  >
                    {label}
                  </span>
                </span>
              </React.Fragment>
            );
          })}
          {hasHistoricalDecision ? (
            <span className="flex min-w-0 items-center gap-1 truncate text-xs text-amber-600 dark:text-amber-400">
              <History className="h-3.5 w-3.5 shrink-0" />
              {t("projects.reviewers.earlierDecision")}
            </span>
          ) : null}
        </div>
        {requestAction}
      </div>
    </ProjectDetailMetaRow>
  );
}
