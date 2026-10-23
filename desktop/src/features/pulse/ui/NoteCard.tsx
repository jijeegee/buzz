import {
  Heart,
  MessageCircle,
  PenSquare,
  SquareArrowOutUpRight,
} from "lucide-react";
import * as React from "react";

import { ForumComposer } from "@/features/forum/ui/ForumComposer";
import { useUserProfileQuery } from "@/features/profile/hooks";
import { AgentBadgeIcon } from "@/features/profile/ui/AgentBadgeIcon";
import { UserProfilePopover } from "@/features/profile/ui/UserProfilePopover";
import { useNoteByIdQuery } from "@/features/pulse/hooks";
import { getReplyParent, noteSnippet } from "@/features/pulse/lib/replies";
import type { UserNote } from "@/shared/api/socialTypes";
import type { ChannelMember, UserProfileSummary } from "@/shared/api/types";
import { AnimatedCount } from "@/shared/ui/AnimatedCount";
import { Markdown } from "@/shared/ui/markdown";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/shared/ui/tooltip";
import { UserAvatar } from "@/shared/ui/UserAvatar";
import { unknownUserLabel } from "@/shared/lib/pubkey";
import { formatLocale, type Translate, useT } from "@/shared/i18n";

export type NoteCardActions = {
  reply?: (
    note: UserNote,
    content: string,
    mentionPubkeys: string[],
    mediaTags?: string[][],
  ) => Promise<unknown>;
  share?: (note: UserNote) => void;
  startDm?: (pubkey: string) => void;
  toggleUpvote?: (note: UserNote, remove: boolean) => Promise<unknown>;
};

type NoteCardProps = {
  note: UserNote;
  profile?: UserProfileSummary | null;
  currentUserDisplayName?: string;
  currentUserProfile?: UserProfileSummary | null;
  composerProfiles?: Record<string, UserProfileSummary>;
  isReplySending?: boolean;
  reactionCount?: number;
  isUpvotePending?: boolean;
  isUpvoted?: boolean;
  members?: ChannelMember[];
  isAgent?: boolean;
  isOwnNote: boolean;
  actions?: NoteCardActions;
};

function ReplyParentContext({
  parentId,
  profiles,
}: {
  parentId: string;
  profiles: Record<string, UserProfileSummary>;
}) {
  const t = useT();
  const parentNoteQuery = useNoteByIdQuery(parentId);
  const parentNote = parentNoteQuery.data ?? null;
  const cachedProfile = parentNote
    ? profiles[parentNote.pubkey.toLowerCase()]
    : null;
  const parentProfileQuery = useUserProfileQuery(
    parentNote && !cachedProfile ? parentNote.pubkey : undefined,
  );
  const fetchedProfile = parentProfileQuery.data ?? null;
  const parentDisplayName = parentNote
    ? (cachedProfile?.displayName ??
      fetchedProfile?.displayName ??
      unknownUserLabel())
    : null;
  const isParentNameLoading =
    !cachedProfile && !fetchedProfile && parentProfileQuery.isLoading;
  const parentAvatarUrl =
    cachedProfile?.avatarUrl ?? fetchedProfile?.avatarUrl ?? null;
  const parentIsAgent = cachedProfile?.isAgent === true;
  const parentSnippet = parentNote ? noteSnippet(parentNote.content) : null;

  return (
    <div className="mt-2 truncate rounded-xl border border-border/50 bg-muted/25 px-3 py-2 text-xs text-muted-foreground">
      {parentNote ? (
        <div className="flex min-w-0 items-center gap-1.5">
          <UserProfilePopover
            pubkey={parentNote.pubkey}
            role={parentIsAgent ? "bot" : undefined}
            triggerElement="span"
          >
            <button
              className="flex shrink-0 rounded-md focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring"
              type="button"
            >
              <UserAvatar
                avatarUrl={parentAvatarUrl}
                className="!h-4 !w-4 shrink-0"
                displayName={
                  parentDisplayName ?? t("pulse.noteCard.parentAuthorFallback")
                }
                shape={parentIsAgent ? "squircle" : "circle"}
              />
            </button>
          </UserProfilePopover>
          <span className="min-w-0 truncate">
            <UserProfilePopover
              pubkey={parentNote.pubkey}
              triggerElement="span"
            >
              <button
                className="rounded font-medium text-foreground/80 focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring"
                type="button"
              >
                {isParentNameLoading ? (
                  <span
                    aria-hidden="true"
                    className="inline-block h-3 w-16 rounded bg-muted align-middle"
                    data-testid="note-parent-author-loading"
                  />
                ) : (
                  parentDisplayName
                )}
              </button>
            </UserProfilePopover>
            : {parentSnippet || t("pulse.noteCard.parentNoText")}
          </span>
        </div>
      ) : parentNoteQuery.isLoading ? (
        t("pulse.noteCard.parentLoading")
      ) : (
        t("pulse.noteCard.parentUnavailable")
      )}
    </div>
  );
}

function formatRelativeTime(t: Translate, unixSeconds: number): string {
  const now = Date.now() / 1_000;
  const diff = now - unixSeconds;

  if (diff < 60) return t("time.justNow");
  if (diff < 3_600)
    return t("pulse.noteCard.minutesShort", { count: Math.floor(diff / 60) });
  if (diff < 86_400)
    return t("pulse.noteCard.hoursShort", { count: Math.floor(diff / 3_600) });
  if (diff < 604_800)
    return t("pulse.noteCard.daysShort", { count: Math.floor(diff / 86_400) });

  return new Date(unixSeconds * 1_000).toLocaleDateString(formatLocale(), {
    month: "short",
    day: "numeric",
  });
}

export function NoteCard({
  note,
  profile,
  currentUserDisplayName,
  currentUserProfile,
  composerProfiles = {},
  isAgent,
  isOwnNote,
  isReplySending = false,
  reactionCount = 0,
  isUpvotePending = false,
  isUpvoted = false,
  members = [],
  actions,
}: NoteCardProps) {
  const t = useT();
  const replyAuthorName = currentUserDisplayName ?? t("common.you");
  const displayName = profile?.displayName ?? unknownUserLabel();
  const avatarUrl = profile?.avatarUrl ?? null;
  const [isReplyComposerOpen, setIsReplyComposerOpen] = React.useState(false);
  const actionButtonClass =
    "inline-flex min-w-7 items-center gap-1.5 text-muted-foreground/60 transition-colors hover:text-foreground focus-visible:text-foreground focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring";
  const activeActionClass = "text-primary";
  const countPlaceholder = <span aria-hidden className="w-2.5" />;
  const reactionCountLabel =
    reactionCount > 0 ? <AnimatedCount value={reactionCount} /> : null;
  const currentUserAvatarUrl = currentUserProfile?.avatarUrl ?? null;
  const replyParentId = getReplyParent(note);

  return (
    <article className="flex items-start gap-2.5 rounded-2xl px-1 pb-1 pt-4 sm:px-2">
      <UserProfilePopover
        botIdenticonValue={displayName}
        pubkey={note.pubkey}
        role={isAgent ? "bot" : undefined}
      >
        <button
          className="relative flex shrink-0 rounded-xl focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring"
          type="button"
        >
          <UserAvatar
            avatarUrl={avatarUrl}
            className="!h-9 !w-9 shrink-0"
            displayName={displayName}
            shape={isAgent ? "squircle" : "circle"}
          />
          {isAgent ? (
            <AgentBadgeIcon
              agentPubkey={note.pubkey}
              className="absolute -bottom-0.5 -right-0.5 h-4 w-4 rounded-full bg-background p-0.5 text-muted-foreground"
              othersSee="bot"
              ownerPubkey={profile?.ownerPubkey}
            />
          ) : null}
        </button>
      </UserProfilePopover>

      <div className="min-w-0 flex-1">
        <div className="flex min-w-0 flex-wrap items-baseline gap-x-2 gap-y-0">
          <UserProfilePopover
            botIdenticonValue={displayName}
            pubkey={note.pubkey}
            role={isAgent ? "bot" : undefined}
          >
            <button
              className="truncate rounded text-sm font-semibold leading-none tracking-tight focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring"
              type="button"
            >
              {displayName}
            </button>
          </UserProfilePopover>
          {isAgent ? (
            <span className="inline-flex h-4 items-center rounded bg-muted px-1 text-2xs font-medium text-muted-foreground">
              {t("pulse.noteCard.botBadge")}
            </span>
          ) : null}
          {profile?.nip05Handle ? (
            <span className="truncate text-xs text-muted-foreground">
              {profile.nip05Handle}
            </span>
          ) : null}
          <span className="shrink-0 text-xs text-muted-foreground/70">
            {formatRelativeTime(t, note.createdAt)}
          </span>
        </div>

        {replyParentId ? (
          <ReplyParentContext
            parentId={replyParentId}
            profiles={composerProfiles}
          />
        ) : null}

        <div className="mt-0.5 pb-3 text-sm text-foreground">
          <Markdown content={note.content} />
        </div>

        <div className="flex flex-wrap items-center gap-5 text-xs font-medium">
          <div className="flex flex-wrap items-center gap-5">
            <Tooltip>
              <TooltipTrigger asChild>
                <button
                  aria-label={
                    isUpvoted
                      ? t("pulse.noteCard.unlike")
                      : t("pulse.noteCard.like")
                  }
                  aria-pressed={isUpvoted}
                  className={`${actionButtonClass} ${isUpvoted ? activeActionClass : ""} disabled:opacity-45`}
                  disabled={isUpvotePending}
                  onClick={() => {
                    if (!isUpvotePending) {
                      void actions?.toggleUpvote?.(note, isUpvoted);
                    }
                  }}
                  type="button"
                >
                  <Heart
                    className={`h-4 w-4 ${isUpvoted ? "fill-current" : ""}`}
                  />
                  {reactionCountLabel}
                </button>
              </TooltipTrigger>
              <TooltipContent>
                {isUpvoted
                  ? t("pulse.noteCard.unlike")
                  : t("pulse.noteCard.like")}
              </TooltipContent>
            </Tooltip>
            <Tooltip>
              <TooltipTrigger asChild>
                <button
                  aria-label={t("pulse.noteCard.reply")}
                  aria-expanded={isReplyComposerOpen}
                  className={actionButtonClass}
                  onClick={() => setIsReplyComposerOpen((current) => !current)}
                  type="button"
                >
                  <MessageCircle className="h-4 w-4" />
                  {countPlaceholder}
                </button>
              </TooltipTrigger>
              <TooltipContent>{t("pulse.noteCard.reply")}</TooltipContent>
            </Tooltip>
            <Tooltip>
              <TooltipTrigger asChild>
                <button
                  aria-label={t("pulse.noteCard.share")}
                  className={actionButtonClass}
                  onClick={() => actions?.share?.(note)}
                  type="button"
                >
                  <SquareArrowOutUpRight className="h-4 w-4" />
                  {countPlaceholder}
                </button>
              </TooltipTrigger>
              <TooltipContent>{t("pulse.noteCard.share")}</TooltipContent>
            </Tooltip>
            {!isOwnNote ? (
              <Tooltip>
                <TooltipTrigger asChild>
                  <button
                    aria-label={t("pulse.noteCard.startDm")}
                    className={actionButtonClass}
                    onClick={() => actions?.startDm?.(note.pubkey)}
                    type="button"
                  >
                    <PenSquare className="h-4 w-4" />
                  </button>
                </TooltipTrigger>
                <TooltipContent>{t("pulse.noteCard.startDm")}</TooltipContent>
              </Tooltip>
            ) : null}
          </div>
        </div>
        {isReplyComposerOpen ? (
          <div className="mt-4 rounded-2xl border border-border/60 bg-background/60 p-3">
            <ForumComposer
              compact
              className="pulse-reply-composer border-0 bg-transparent p-0 shadow-none"
              disabled={!actions?.reply}
              header={
                <div className="flex min-w-0 items-center gap-2">
                  <UserAvatar
                    avatarUrl={currentUserAvatarUrl}
                    className="!h-8 !w-8 shrink-0"
                    displayName={replyAuthorName}
                    shape={currentUserProfile?.isAgent ? "squircle" : "circle"}
                  />
                  <span className="max-w-32 truncate text-sm font-medium text-foreground">
                    {replyAuthorName}
                  </span>
                </div>
              }
              isSending={isReplySending}
              members={members}
              onCancel={() => setIsReplyComposerOpen(false)}
              onSubmit={(content, mentionPubkeys, mediaTags) =>
                actions
                  ?.reply?.(note, content, mentionPubkeys, mediaTags)
                  ?.then(() => {
                    setIsReplyComposerOpen(false);
                  })
              }
              placeholder={t("pulse.noteCard.replyPlaceholder")}
              profiles={composerProfiles}
            />
          </div>
        ) : null}
      </div>
    </article>
  );
}
