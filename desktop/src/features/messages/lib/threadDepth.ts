/**
 * Threads are one level deep (Slack-style): "Reply in thread" is offered only
 * on messages shallower than this, so replies inside a thread stay flat.
 * Existing deeper replies still render. Raise it to allow nested replies again.
 */
export const MAX_THREAD_DEPTH = 1;

export function canReplyInThread(depth: number): boolean {
  return depth < MAX_THREAD_DEPTH;
}
