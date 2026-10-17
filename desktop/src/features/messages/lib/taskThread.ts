/** Close marker written by `kuzz threads close`, in the thread and on its result. */
export const THREAD_CLOSED_TAG = "buzz:thread-closed";
/** Reopen marker written by `kuzz threads reopen` and the Reopen button. */
export const THREAD_REOPENED_TAG = "buzz:thread-reopened";

/** Marker reply text for the Close toggle; reopen matches `kuzz threads reopen`. */
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

function hasThreadReopenedTag(
  tags: readonly (readonly string[])[] | null | undefined,
): boolean {
  return (
    tags?.some((tag) => tag[0] === THREAD_REOPENED_TAG && Boolean(tag[1])) ??
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
  // Within one second the later reply in list order wins (the relay's
  // order: a final reply and `kuzz threads close` in one second read as
  // closed), except that a close marker never beats a reopen marker from the
  // same second, so an Undo right after Close reads as open in any order.
  let latest: T | null = null;
  for (const reply of replies) {
    if (
      !latest ||
      reply.createdAt > latest.createdAt ||
      (reply.createdAt === latest.createdAt &&
        !(hasThreadReopenedTag(latest.tags) && hasThreadClosedTag(reply.tags)))
    ) {
      latest = reply;
    }
  }
  if (!latest || !hasThreadClosedTag(latest.tags)) {
    return { closed: false, closedBy: null, marker: null };
  }
  return { closed: true, closedBy: latest.pubkey ?? null, marker: latest };
}
