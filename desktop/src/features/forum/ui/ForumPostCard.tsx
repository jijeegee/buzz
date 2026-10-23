import { AlertCircle, MessageSquare } from "lucide-react";
import { useMemo } from "react";

import {
  resolveUserLabel,
  type UserProfileLookup,
} from "@/features/profile/lib/identity";
import { UserProfilePopover } from "@/features/profile/ui/UserProfilePopover";
import { UserAvatar } from "@/shared/ui/UserAvatar";
import type { ForumPost } from "@/shared/api/types";
import { useT } from "@/shared/i18n";
import { cn } from "@/shared/lib/cn";
import { resolveMentionProps } from "@/shared/lib/resolveMentionNames";
import { Markdown } from "@/shared/ui/markdown";
import { hasLinkPreviewSuppression } from "@/features/messages/lib/formatTimelineMessages";
import {
  FailedSendActions,
  MessageSendState,
} from "@/features/messages/ui/MessageSendState";
import { parseImetaTags } from "@/shared/ui/markdown/parseImeta";

import { formatRelativeTime } from "../lib/time";
import { DeleteActionMenu } from "./DeleteActionMenu";

type ForumPostCardProps = {
  post: ForumPost;
  currentPubkey?: string;
  profiles?: UserProfileLookup;
  isActive?: boolean;
  canDelete?: boolean;
  isDeleting?: boolean;
  /** Own replies to this post the relay did not accept. */
  failedReplyCount?: number;
  onClick: (post: ForumPost) => void;
  onDelete?: (eventId: string) => void;
};

export function ForumPostCard({
  post,
  currentPubkey,
  profiles,
  isActive,
  canDelete,
  isDeleting,
  failedReplyCount = 0,
  onClick,
  onDelete,
}: ForumPostCardProps) {
  const t = useT();
  const authorLabel = resolveUserLabel({
    pubkey: post.pubkey,
    currentPubkey,
    profiles,
    preferResolvedSelfLabel: true,
  });
  const avatarUrl = profiles?.[post.pubkey.toLowerCase()]?.avatarUrl ?? null;
  const authorIsAgent = profiles?.[post.pubkey.toLowerCase()]?.isAgent === true;
  const { mentionNames, mentionPubkeysByName } = resolveMentionProps(
    post.tags,
    profiles,
    post.content,
  );
  // Memoize the imeta map: `parseImetaTags` builds a fresh object each render,
  // and the `Markdown` memo compares `imetaByUrl` by reference. Without this,
  // the post's Markdown (and the FileCard <button> it renders) is rebuilt on
  // every ForumPostCard render, swapping the live DOM node. A click that lands
  // across one of those swaps splits mousedown/mouseup onto different nodes, so
  // the browser never fires `click` and a file download is silently dropped.
  const imetaByUrl = useMemo(() => parseImetaTags(post.tags), [post.tags]);
  const summary = post.threadSummary;
  const previewContent =
    post.content.length > 200
      ? `${post.content.slice(0, 200)}...`
      : post.content;

  return (
    // biome-ignore lint/a11y/useSemanticElements: Cannot use <button> because DeleteActionMenu renders a nested <button> via DropdownMenuTrigger, which is invalid HTML
    <div
      role="button"
      tabIndex={0}
      className={cn(
        "group w-full cursor-pointer rounded-xl border border-border/60 bg-card p-4 text-left transition-colors hover:border-border hover:bg-accent/40",
        isActive && "border-primary/40 bg-accent/60",
        isDeleting && "pointer-events-none opacity-50",
      )}
      onClick={() => onClick(post)}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onClick(post);
        }
      }}
    >
      <div className="flex items-center gap-2">
        {/* biome-ignore lint/a11y/noStaticElementInteractions: presentation wrapper stops click propagation to parent card */}
        <div onClick={(e) => e.stopPropagation()} role="presentation">
          <UserProfilePopover
            pubkey={post.pubkey}
            role={authorIsAgent ? "bot" : undefined}
          >
            <button
              className="flex items-center gap-2 rounded-lg focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring"
              type="button"
            >
              <UserAvatar
                accent={authorIsAgent}
                avatarUrl={avatarUrl}
                displayName={authorLabel}
                shape={authorIsAgent ? "squircle" : "circle"}
                size="sm"
              />
              <span className="truncate text-sm font-medium text-foreground hover:underline">
                {authorLabel}
              </span>
            </button>
          </UserProfilePopover>
        </div>
        <span className="text-xs text-muted-foreground">
          {formatRelativeTime(post.createdAt)}
        </span>
        {post.pending ? (
          <MessageSendState failed={post.failed === true} />
        ) : null}

        {canDelete && onDelete ? (
          // biome-ignore lint/a11y/noStaticElementInteractions: presentation wrapper only stops click propagation to parent card link
          <div
            className="ml-auto"
            onClick={(e) => e.stopPropagation()}
            role="presentation"
          >
            <DeleteActionMenu
              label="post"
              onConfirm={() => onDelete(post.eventId)}
            />
          </div>
        ) : null}
      </div>

      <div className="mt-2">
        <Markdown
          className="text-sm"
          content={previewContent}
          messageId={post.eventId}
          linkPreviewsSuppressed={hasLinkPreviewSuppression(post.tags)}
          linkPreviewTags={post.tags}
          imetaByUrl={imetaByUrl}
          mentionNames={mentionNames}
          mentionPubkeysByName={mentionPubkeysByName}
        />
      </div>

      {post.failed && post.localKey ? (
        // biome-ignore lint/a11y/noStaticElementInteractions: presentation wrapper stops click propagation to parent card
        <div
          className="mt-2 text-xs"
          onClick={(e) => e.stopPropagation()}
          role="presentation"
        >
          <FailedSendActions localKey={post.localKey} />
        </div>
      ) : null}

      {failedReplyCount > 0 ? (
        <div
          className="mt-3 flex items-center gap-1.5 text-xs text-destructive"
          data-testid="forum-post-replies-not-sent"
        >
          <AlertCircle aria-hidden="true" className="h-3.5 w-3.5" />
          <span>
            {t("forum.postCard.repliesNotSent", { count: failedReplyCount })}
          </span>
        </div>
      ) : null}

      {summary && summary.replyCount > 0 ? (
        <div className="mt-3 flex items-center gap-1.5 text-xs text-muted-foreground">
          <MessageSquare className="h-4 w-4" />
          <span>
            {t("forum.postCard.replyCount", { count: summary.replyCount })}
          </span>
          {summary.lastReplyAt ? (
            <>
              <span className="text-muted-foreground/50">·</span>
              <span>
                {t("forum.postCard.lastReply", {
                  time: formatRelativeTime(summary.lastReplyAt),
                })}
              </span>
            </>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
