const THREAD_CONTEXT_PREFIX = "thread:";

/**
 * Keeps a thread's "Mark as unread" from being undone by the open panel.
 * Read positions keep advancing while the thread is on screen, and each one
 * would send a read that clears the relay's unread mark at once. After a mark,
 * reads of messages from before it are not sent; reading a newer message
 * (one that arrived after the mark) or an explicit Mark as read lifts the
 * guard, as the message menu's mark-unread lasts until there is something
 * new.
 */
export class ThreadUnreadGuard {
  private readonly markedAt = new Map<string, number>();

  /** `nowSeconds`: unix seconds when the user marked the thread unread. */
  mark(rootId: string, nowSeconds: number): void {
    this.markedAt.set(rootId, Math.floor(nowSeconds));
  }

  /** An explicit Mark as read: the next read goes through. */
  clear(rootId: string): void {
    this.markedAt.delete(rootId);
  }

  reset(): void {
    this.markedAt.clear();
  }

  /** Whether a read of `contextId` up to `at` (unix seconds) may be sent. */
  allowRead(contextId: string, at: number): boolean {
    if (!contextId.startsWith(THREAD_CONTEXT_PREFIX)) return true;
    const rootId = contextId.slice(THREAD_CONTEXT_PREFIX.length);
    const markedAt = this.markedAt.get(rootId);
    if (markedAt === undefined) return true;
    if (at <= markedAt) return false;
    this.markedAt.delete(rootId);
    return true;
  }
}
