import type { QueryClient } from "@tanstack/react-query";

import {
  isStoredSendCopy,
  type StoredSendRelayCheck,
} from "@/features/messages/hooks";
import {
  forgetFailedSend,
  hasFailedSend,
  registerFailedSend,
} from "@/features/messages/lib/failedSends";
import {
  currentStoredSendKey,
  isStoredSendInFlight,
  markStoredSendInFlight,
  removeStoredSend,
  storedSends,
} from "@/features/messages/lib/failedSendStore";
import { normalizeMentionPubkeys } from "@/features/messages/lib/threading";
import type {
  ForumPost,
  ForumPostsResponse,
  ForumThreadResponse,
  RelayEvent,
  ThreadReply,
} from "@/shared/api/types";
import { KIND_FORUM_COMMENT, KIND_FORUM_POST } from "@/shared/constants/kinds";

/**
 * Own forum posts and replies drawn the moment they are sent. They live in
 * their own cache, merged into the fetched lists when rendered, because the
 * forum queries poll and would wipe rows written into them. A row goes once
 * a fetch returns its event.
 */
export type ForumLocalSend = {
  localKey: string;
  channelId: string;
  /** The post a reply answers; null for a new post. */
  parentEventId: string | null;
  /** The row as drawn: its second is the one the send is signed at. */
  event: RelayEvent;
  /** The relay's id once it accepted the send. */
  eventId: string | null;
  failed: boolean;
};

/** What a forum send carries; Retry sends the same again. */
export type ForumSendVariables = {
  localKey: string;
  channelId: string;
  kind: typeof KIND_FORUM_POST | typeof KIND_FORUM_COMMENT;
  content: string;
  parentEventId: string | null;
  mentionPubkeys: string[];
  mediaTags: string[][];
};

export function forumLocalSendsKey(channelId: string) {
  return ["forum-local-sends", channelId] as const;
}

export function readForumLocalSends(
  queryClient: QueryClient,
  channelId: string,
): ForumLocalSend[] {
  return (
    queryClient.getQueryData<ForumLocalSend[]>(forumLocalSendsKey(channelId)) ??
    []
  );
}

export function findForumLocalSend(
  queryClient: QueryClient,
  channelId: string,
  localKey: string,
): ForumLocalSend | undefined {
  return readForumLocalSends(queryClient, channelId).find(
    (send) => send.localKey === localKey,
  );
}

/** Add a row, or replace the one with the same local key in place. */
export function writeForumLocalSend(
  queryClient: QueryClient,
  send: ForumLocalSend,
) {
  const current = readForumLocalSends(queryClient, send.channelId);
  const index = current.findIndex((row) => row.localKey === send.localKey);
  queryClient.setQueryData<ForumLocalSend[]>(
    forumLocalSendsKey(send.channelId),
    index === -1
      ? [...current, send]
      : current.map((row, i) => (i === index ? send : row)),
  );
}

export function updateForumLocalSend(
  queryClient: QueryClient,
  channelId: string,
  localKey: string,
  update: (send: ForumLocalSend) => ForumLocalSend,
) {
  const row = findForumLocalSend(queryClient, channelId, localKey);
  if (row) writeForumLocalSend(queryClient, update(row));
}

export function removeForumLocalSend(
  queryClient: QueryClient,
  channelId: string,
  localKey: string,
) {
  const current = readForumLocalSends(queryClient, channelId);
  if (!current.some((row) => row.localKey === localKey)) return;
  queryClient.setQueryData<ForumLocalSend[]>(
    forumLocalSendsKey(channelId),
    current.filter((row) => row.localKey !== localKey),
  );
}

/** Drop the row of an accepted send whose event was deleted. */
export function removeForumLocalSendOfEvent(
  queryClient: QueryClient,
  channelId: string,
  eventId: string,
) {
  const current = readForumLocalSends(queryClient, channelId);
  if (!current.some((row) => row.eventId === eventId)) return;
  queryClient.setQueryData<ForumLocalSend[]>(
    forumLocalSendsKey(channelId),
    current.filter((row) => row.eventId !== eventId),
  );
}

/** The event as the relay will store it, so its copy can be recognised. */
export function buildForumLocalEvent(
  variables: ForumSendVariables,
  pubkey: string,
  createdAt: number,
): RelayEvent {
  const tags: string[][] = [["h", variables.channelId]];
  if (variables.parentEventId) {
    tags.push(["e", variables.parentEventId, "", "reply"]);
  }
  for (const mention of normalizeMentionPubkeys(
    variables.mentionPubkeys,
    pubkey,
  )) {
    tags.push(["p", mention]);
  }
  tags.push(...variables.mediaTags);
  return {
    id: variables.localKey,
    localKey: variables.localKey,
    pubkey,
    created_at: createdAt,
    kind: variables.kind,
    tags,
    content: variables.content.trim(),
    sig: "",
    pending: true,
  };
}

function postEvent(post: ForumPost): RelayEvent {
  return {
    id: post.eventId,
    pubkey: post.pubkey,
    created_at: post.createdAt,
    kind: post.kind,
    tags: post.tags,
    content: post.content,
    sig: "",
  };
}

function replyEvent(reply: ThreadReply): RelayEvent {
  return {
    id: reply.eventId,
    pubkey: reply.pubkey,
    created_at: reply.createdAt,
    kind: reply.kind,
    tags: reply.tags,
    content: reply.content,
    sig: "",
  };
}

/** Every forum event of the channel the loaded posts and threads hold. */
export function fetchedForumEvents(
  queryClient: QueryClient,
  channelId: string,
): RelayEvent[] {
  const posts = queryClient
    .getQueriesData<ForumPostsResponse>({
      queryKey: ["forum-posts", channelId],
    })
    .flatMap(([, data]) => (data ? data.posts.map(postEvent) : []));
  const threads = queryClient
    .getQueriesData<ForumThreadResponse>({
      queryKey: ["forum-thread", channelId],
    })
    .flatMap(([, data]) =>
      data ? [postEvent(data.post), ...data.replies.map(replyEvent)] : [],
    );
  return [...posts, ...threads];
}

/** Whether `fetched` holds the relay's copy of a local row. */
function isFetched(send: ForumLocalSend, fetched: RelayEvent[]): boolean {
  if (send.eventId) return fetched.some((event) => event.id === send.eventId);
  const match = { ...send.event, pending: true };
  return fetched.some((event) => isStoredSendCopy(match, event));
}

/** Whether the relay's copy of an own send, as drawn, is already loaded. */
export function hasForumCopy(
  queryClient: QueryClient,
  channelId: string,
  drawn: RelayEvent,
): boolean {
  const match = { ...drawn, pending: true };
  return fetchedForumEvents(queryClient, channelId).some((event) =>
    isStoredSendCopy(match, event),
  );
}

function sendState(send: ForumLocalSend) {
  return {
    localKey: send.localKey,
    pending: send.eventId === null,
    failed: send.failed,
  };
}

/**
 * Put each row among `items` by time, never reordering what was fetched.
 * `newestFirst` lists put a row above anything older or of the same second.
 */
function insertByTime<T extends { createdAt: number }>(
  items: T[],
  rows: T[],
  newestFirst: boolean,
): T[] {
  if (rows.length === 0) return items;
  const result = [...items];
  const sorted = [...rows].sort((a, b) =>
    newestFirst ? b.createdAt - a.createdAt : a.createdAt - b.createdAt,
  );
  let from = 0;
  for (const row of sorted) {
    let index = result.findIndex(
      (item, i) =>
        i >= from &&
        (newestFirst
          ? item.createdAt <= row.createdAt
          : item.createdAt > row.createdAt),
    );
    if (index === -1) index = result.length;
    result.splice(index, 0, row);
    from = index + 1;
  }
  return result;
}

/** Fetched posts with own posts not yet among them, newest first. */
export function mergeForumPosts(
  posts: ForumPost[],
  sends: ForumLocalSend[],
): ForumPost[] {
  const fetched = posts.map(postEvent);
  const rows = sends
    .filter((send) => send.parentEventId === null && !isFetched(send, fetched))
    .map(
      (send): ForumPost => ({
        eventId: send.eventId ?? send.localKey,
        pubkey: send.event.pubkey,
        content: send.event.content,
        kind: send.event.kind,
        createdAt: send.event.created_at,
        channelId: send.channelId,
        tags: send.event.tags,
        threadSummary: null,
        ...sendState(send),
      }),
    );
  return insertByTime(posts, rows, true);
}

/** Fetched replies with own replies to `postId` not yet among them. */
export function mergeForumReplies(
  replies: ThreadReply[],
  sends: ForumLocalSend[],
  postId: string,
): ThreadReply[] {
  const fetched = replies.map(replyEvent);
  const rows = sends
    .filter(
      (send) => send.parentEventId === postId && !isFetched(send, fetched),
    )
    .map(
      (send): ThreadReply => ({
        eventId: send.eventId ?? send.localKey,
        pubkey: send.event.pubkey,
        content: send.event.content,
        kind: send.event.kind,
        createdAt: send.event.created_at,
        channelId: send.channelId,
        tags: send.event.tags,
        parentEventId: postId,
        rootEventId: postId,
        depth: 1,
        ...sendState(send),
      }),
    );
  return insertByTime(replies, rows, false);
}

function isForumKind(kind: number) {
  return kind === KIND_FORUM_POST || kind === KIND_FORUM_COMMENT;
}

/**
 * Keep own forum rows in step with what the forum has loaded: a row whose
 * event a fetch returned goes, and sends stored on disk that the relay never
 * acknowledged (from an earlier app session) come back marked failed with
 * Retry / Delete. Mirrors `restoreStoredSends` for chat: while the relay is
 * asked whether a stored send landed, it is not drawn, so a delivered post
 * never shows as failed and Retry can never post it twice.
 */
export function syncForumLocalSends(
  queryClient: QueryClient,
  channelId: string,
  retry: (variables: ForumSendVariables) => Promise<unknown>,
  relayCheck?: StoredSendRelayCheck,
) {
  const fetched = fetchedForumEvents(queryClient, channelId);
  for (const send of readForumLocalSends(queryClient, channelId)) {
    if (!isFetched(send, fetched)) continue;
    removeForumLocalSend(queryClient, channelId, send.localKey);
    if (!isStoredSendInFlight(send.localKey)) {
      // Also a failed row whose send landed after all (it timed out).
      forgetFailedSend(send.localKey);
      removeStoredSend(send.localKey);
    }
  }

  // Wait for the posts list, as chat waits for its timeline, so a delivered
  // send is recognised before it could be drawn as failed.
  const hasPosts = queryClient
    .getQueriesData<ForumPostsResponse>({
      queryKey: ["forum-posts", channelId],
    })
    .some(([, data]) => data !== undefined);
  if (!hasPosts) return;

  for (const stored of storedSends(channelId)) {
    const { localKey } = stored;
    if (!isForumKind(stored.event.kind) || isStoredSendInFlight(localKey)) {
      continue;
    }
    // Drawn in this session already: its own flow owns Retry / Delete.
    if (findForumLocalSend(queryClient, channelId, localKey)) continue;
    const match: RelayEvent = { ...stored.event, pending: true };
    if (fetched.some((event) => isStoredSendCopy(match, event))) {
      forgetFailedSend(localKey);
      removeStoredSend(localKey);
      continue;
    }
    if (relayCheck) {
      const checked = relayCheck.state.get(localKey);
      if (checked === undefined) {
        relayCheck.state.set(localKey, "checking");
        const scopeKey = currentStoredSendKey();
        relayCheck
          .lookup(stored)
          .then((events) => {
            relayCheck.state.set(localKey, "absent");
            if (
              !relayCheck.isDisposed() &&
              !isStoredSendInFlight(localKey) &&
              events.some((event) => isStoredSendCopy(match, event))
            ) {
              removeStoredSend(localKey, scopeKey);
            }
          })
          // Unreachable relay: show it failed so Retry / Delete stay possible.
          .catch(() => {
            relayCheck.state.set(localKey, "unverified");
          })
          .finally(() => relayCheck.onSettled());
      }
      if (checked === undefined || checked === "checking") continue;
    }
    const parentEventId = stored.send.parentEventId;
    writeForumLocalSend(queryClient, {
      localKey,
      channelId,
      parentEventId,
      event: stored.event,
      eventId: null,
      failed: true,
    });
    if (hasFailedSend(localKey)) continue;
    const discard = () => {
      removeForumLocalSend(queryClient, channelId, localKey);
      removeStoredSend(localKey);
    };
    registerFailedSend(localKey, {
      retry: () => {
        // In flight before the registry entry goes, so a cache update in
        // between cannot arm a second Retry.
        markStoredSendInFlight(localKey);
        const send = () =>
          retry({
            localKey,
            channelId,
            kind:
              stored.event.kind === KIND_FORUM_POST
                ? KIND_FORUM_POST
                : KIND_FORUM_COMMENT,
            content: stored.send.content,
            parentEventId,
            mentionPubkeys: stored.send.mentionPubkeys,
            mediaTags: stored.send.mediaTags,
          }).catch(() => {});
        if (relayCheck?.state.get(localKey) !== "unverified") {
          send();
          return;
        }
        // The relay could not be asked at restore: ask now, so a post that
        // landed before the app quit is not posted twice.
        relayCheck
          .lookup(stored)
          .then((events) => {
            if (!events.some((event) => isStoredSendCopy(match, event))) {
              return send();
            }
            discard();
          })
          .catch(send);
      },
      discard,
    });
  }
}
