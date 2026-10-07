/** Close marker written by `buzz threads close`, in the thread and on its result. */
export const THREAD_CLOSED_TAG = "buzz:thread-closed";

type ThreadMessageLike = {
  createdAt: number;
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
  let latest: ThreadMessageLike | null = null;
  for (const reply of replies) {
    if (!latest || reply.createdAt >= latest.createdAt) latest = reply;
  }
  return latest ? hasThreadClosedTag(latest.tags) : false;
}
