import { MessageSquareText } from "lucide-react";
import * as React from "react";

import { handleTimelineMentionCopy } from "@/features/messages/lib/timelineMentionCopy";
import { useProfileQuery, useUsersBatchQuery } from "@/features/profile/hooks";
import { mergeCurrentProfileIntoLookup } from "@/features/profile/lib/identity";
import { getMentionTagPubkey } from "@/shared/lib/resolveMentionNames";
import type { Channel } from "@/shared/api/types";
import { channelChrome } from "@/shared/layout/chromeLayout";
import { useT } from "@/shared/i18n";
import { KIND_FORUM_COMMENT, KIND_FORUM_POST } from "@/shared/constants/kinds";
import { cn } from "@/shared/lib/cn";
import { Skeleton } from "@/shared/ui/skeleton";
import { VirtualizedList } from "@/shared/ui/VirtualizedList";

import {
  useDeleteForumPostMutation,
  useDeleteForumReplyMutation,
  useForumLocalSends,
  useForumPostsQuery,
  useForumSend,
  useForumThreadQuery,
} from "../hooks";
import { mergeForumPosts, mergeForumReplies } from "../lib/localSends";
import { ForumComposer } from "./ForumComposer";
import { ForumPostCard } from "./ForumPostCard";
import { ForumThreadPanel } from "./ForumThreadPanel";

type ForumViewProps = {
  channel: Channel;
  currentPubkey?: string;
  onClosePost: () => void;
  onSelectPost: (postId: string) => void;
  onTargetReached?: (messageId: string) => void;
  selectedPostId: string | null;
  targetReplyId: string | null;
  targetSearchMessageId?: string;
  targetSearchQuery?: string;
};

function canDelete(postPubkey: string, currentPubkey?: string): boolean {
  if (!currentPubkey) return false;
  // Author can always delete their own posts. Admin check would need
  // channel member role data — for now, author-only is sufficient.
  return postPubkey.toLowerCase() === currentPubkey.toLowerCase();
}

export function ForumView({
  channel,
  currentPubkey,
  onClosePost,
  onSelectPost,
  onTargetReached,
  selectedPostId,
  targetReplyId,
  targetSearchMessageId,
  targetSearchQuery,
}: ForumViewProps) {
  const t = useT();
  const [isComposerOpen, setIsComposerOpen] = React.useState(false);
  const postsScrollRef = React.useRef<HTMLDivElement>(null);

  const profileQuery = useProfileQuery();
  const postsQuery = useForumPostsQuery(channel);
  const threadQuery = useForumThreadQuery(
    selectedPostId ? channel.id : null,
    selectedPostId,
  );
  const effectiveCurrentPubkey = currentPubkey ?? profileQuery.data?.pubkey;
  const forumSend = useForumSend(effectiveCurrentPubkey);
  const localSends = useForumLocalSends(channel.id, forumSend.retry);
  const deletePostMutation = useDeleteForumPostMutation(channel);
  const deleteReplyMutation = useDeleteForumReplyMutation(
    channel,
    selectedPostId,
  );

  // Own posts and replies show the moment they are sent, merged into what
  // the forum fetched until a fetch returns them.
  const fetchedPosts = postsQuery.data?.posts;
  const posts = React.useMemo(
    () => mergeForumPosts(fetchedPosts ?? [], localSends),
    [fetchedPosts, localSends],
  );
  // A failed reply shows only in its thread; its post card says so.
  const failedReplyCounts = React.useMemo(() => {
    const counts = new Map<string, number>();
    for (const send of localSends) {
      if (send.failed && send.parentEventId) {
        counts.set(
          send.parentEventId,
          (counts.get(send.parentEventId) ?? 0) + 1,
        );
      }
    }
    return counts;
  }, [localSends]);
  const fetchedThread = threadQuery.data;
  const thread = React.useMemo(
    () =>
      fetchedThread && selectedPostId
        ? {
            ...fetchedThread,
            replies: mergeForumReplies(
              fetchedThread.replies,
              localSends,
              selectedPostId,
            ),
          }
        : fetchedThread,
    [fetchedThread, localSends, selectedPostId],
  );

  // Collect all pubkeys from posts and thread for profile resolution.
  // Mentioned pubkeys (`p`/`mention` tags) must be included too: mention
  // chips resolve names from this same lookup, and a mentioned user who
  // never authored a post would otherwise render as a dead chip.
  const allPubkeys = React.useMemo(() => {
    const pubkeys = new Set<string>();
    const addMentionPubkeys = (tags?: string[][]) => {
      for (const tag of tags ?? []) {
        const pubkey = getMentionTagPubkey(tag);
        if (pubkey) {
          pubkeys.add(pubkey);
        }
      }
    };
    for (const post of posts) {
      pubkeys.add(post.pubkey);
      addMentionPubkeys(post.tags);
      if (post.threadSummary?.participants) {
        for (const pk of post.threadSummary.participants) {
          pubkeys.add(pk);
        }
      }
    }
    if (thread) {
      pubkeys.add(thread.post.pubkey);
      addMentionPubkeys(thread.post.tags);
      for (const reply of thread.replies) {
        pubkeys.add(reply.pubkey);
        addMentionPubkeys(reply.tags);
      }
    }
    return [...pubkeys];
  }, [posts, thread]);

  const profilesQuery = useUsersBatchQuery(allPubkeys, {
    enabled: allPubkeys.length > 0,
  });
  const profiles = React.useMemo(
    () =>
      mergeCurrentProfileIntoLookup(
        profilesQuery.data?.profiles,
        profileQuery.data,
      ),
    [profileQuery.data, profilesQuery.data?.profiles],
  );

  const previousChannelIdRef = React.useRef(channel.id);
  React.useEffect(() => {
    if (previousChannelIdRef.current === channel.id) {
      return;
    }

    previousChannelIdRef.current = channel.id;
    setIsComposerOpen(false);
  }, [channel.id]);

  if (selectedPostId) {
    const threadPost = thread?.post;
    const canDeleteExpandedPost = threadPost
      ? canDelete(threadPost.pubkey, effectiveCurrentPubkey)
      : false;

    return (
      <ForumThreadPanel
        key={`${channel.id}:${selectedPostId}`}
        postId={selectedPostId}
        canDeletePost={canDeleteExpandedPost}
        currentPubkey={effectiveCurrentPubkey}
        isDeletingPost={deletePostMutation.isPending}
        isLoading={threadQuery.isLoading}
        onBack={onClosePost}
        onDeletePost={(eventId) => {
          deletePostMutation.mutate({ eventId }, { onSuccess: onClosePost });
        }}
        onDeleteReply={(eventId) => {
          deleteReplyMutation.mutate({ eventId });
        }}
        channelId={channel.id}
        onReply={(content, mentionPubkeys, mediaTags) => {
          // The reply is drawn in the thread; the composer is free at once.
          forumSend.send({
            channelId: channel.id,
            kind: KIND_FORUM_COMMENT,
            content,
            parentEventId: selectedPostId,
            mentionPubkeys,
            mediaTags: mediaTags ?? [],
          });
        }}
        onTargetReached={onTargetReached}
        profiles={profiles}
        targetEventId={targetReplyId}
        targetSearchMessageId={targetSearchMessageId}
        targetSearchQuery={targetSearchQuery}
        thread={thread}
      />
    );
  }

  return (
    <div className={cn("flex h-full flex-col", channelChrome.contentPadding)}>
      <div className="border-b border-border/60 p-4">
        {isComposerOpen ? (
          <ForumComposer
            autocompleteBelow
            channelId={channel.id}
            channelType="forum"
            draftKey={`forum:${channel.id}`}
            onCancel={() => setIsComposerOpen(false)}
            onSubmit={(content, mentionPubkeys, mediaTags) => {
              // The post is drawn in the list; the composer closes at once.
              forumSend.send({
                channelId: channel.id,
                kind: KIND_FORUM_POST,
                content,
                parentEventId: null,
                mentionPubkeys,
                mediaTags: mediaTags ?? [],
              });
              setIsComposerOpen(false);
            }}
            placeholder={t("forum.view.postPlaceholder")}
            profiles={profiles}
          />
        ) : (
          <button
            className="w-full rounded-xl border border-dashed border-border/80 px-4 py-3 text-left text-sm text-muted-foreground transition-colors hover:border-border hover:bg-accent/30 hover:text-foreground"
            disabled={!channel.isMember || channel.archivedAt !== null}
            onClick={() => setIsComposerOpen(true)}
            type="button"
          >
            {channel.archivedAt
              ? t("forum.view.archived")
              : !channel.isMember
                ? t("forum.view.joinToPost")
                : t("forum.view.startPost")}
          </button>
        )}
      </div>

      <div
        className="flex-1 overflow-y-auto"
        data-scroll-restoration-id={`forum-list:${channel.id}`}
        onCopy={handleTimelineMentionCopy}
        ref={postsScrollRef}
      >
        {postsQuery.isLoading ? (
          <div className="space-y-3 p-4">
            <Skeleton className="h-24 w-full rounded-xl" />
            <Skeleton className="h-24 w-full rounded-xl" />
            <Skeleton className="h-24 w-full rounded-xl" />
          </div>
        ) : posts.length === 0 ? (
          <div className="flex flex-col items-center justify-center gap-3 px-4 py-16 text-center">
            <MessageSquareText className="h-10 w-10 text-muted-foreground/40" />
            <div>
              <p className="text-sm font-medium text-foreground/70">
                {t("forum.view.emptyTitle")}
              </p>
              <p className="mt-1 text-xs text-muted-foreground">
                {t("forum.view.emptyBody")}
              </p>
            </div>
          </div>
        ) : (
          <VirtualizedList
            estimateSize={120}
            getItemKey={(post) => post.localKey ?? post.eventId}
            innerClassName="p-4"
            items={posts}
            renderItem={(post) => (
              <div className="pb-3">
                <ForumPostCard
                  canDelete={
                    !post.pending &&
                    canDelete(post.pubkey, effectiveCurrentPubkey)
                  }
                  currentPubkey={effectiveCurrentPubkey}
                  isActive={selectedPostId === post.eventId}
                  isDeleting={
                    deletePostMutation.isPending &&
                    deletePostMutation.variables?.eventId === post.eventId
                  }
                  failedReplyCount={failedReplyCounts.get(post.eventId) ?? 0}
                  onClick={() => {
                    // Own posts no fetch has returned yet: the relay may not
                    // serve the thread, or not even hold the post.
                    if (!post.localKey) onSelectPost(post.eventId);
                  }}
                  onDelete={(eventId) => {
                    deletePostMutation.mutate({ eventId });
                  }}
                  post={post}
                  profiles={profiles}
                />
              </div>
            )}
            scrollRef={postsScrollRef}
          />
        )}
      </div>
    </div>
  );
}
