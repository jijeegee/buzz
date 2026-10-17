/** Close marker written by `kuzz threads close`, in the thread and on its result. */
export const THREAD_CLOSED_TAG = "buzz:thread-closed";
/** Reopen marker written by `kuzz threads reopen` and the Reopen button. */
export const THREAD_REOPENED_TAG = "buzz:thread-reopened";

/** Marker reply text, the same as `kuzz threads close` / `reopen` write. */
export const THREAD_CLOSED_CONTENT = "✅ Thread closed.";
export const THREAD_REOPENED_CONTENT = "Thread reopened.";

type ThreadMessageLike = {
  createdAt: number;
  pubkey?: string;
  tags?: readonly (readonly string[])[];
};

export function hasThreadClosedTag(
  tags: readonly (readonly string[])[] | null | undefined,
): boolean {
  return (
    tags?.some((tag) => tag[0] === THREAD_CLOSED_TAG && Boolean(tag[1])) ??
    false
  );
}

/**
 * A task thread is closed while its latest reply is a close marker. Any later
 * message reopens it, so no separate state event is needed.
 */
export function isTaskThreadClosed(
  replies: readonly ThreadMessageLike[],
): boolean {
  return threadCloseState(replies).closed;
}

export type ThreadCloseState<T extends ThreadMessageLike = ThreadMessageLike> =
  {
    closed: boolean;
    /** Who wrote the close marker, while closed. */
    closedBy: string | null;
    /** The close marker reply itself, while closed. */
    marker: T | null;
  };

/** The Close toggle's state: closed while the latest reply is a close marker. */
export function threadCloseState<T extends ThreadMessageLike>(
  replies: readonly T[],
): ThreadCloseState<T> {
  // Within one second (an Undo right after Close), any other message wins:
  // a thread reads as open unless a close marker is strictly the latest.
  let latest: T | null = null;
  for (const reply of replies) {
    if (
      !latest ||
      reply.createdAt > latest.createdAt ||
      (reply.createdAt === latest.createdAt && !hasThreadClosedTag(reply.tags))
    ) {
      latest = reply;
    }
  }
  if (!latest || !hasThreadClosedTag(latest.tags)) {
    return { closed: false, closedBy: null, marker: null };
  }
  return { closed: true, closedBy: latest.pubkey ?? null, marker: latest };
}
