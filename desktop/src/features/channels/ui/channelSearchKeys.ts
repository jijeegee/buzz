/**
 * URL search-param keys managed by the channel-panel history state.
 *
 * Kept in a separate pure module (no React / router imports) so tests can
 * import and assert the patch contracts without a browser environment.
 */

export const CHANNEL_SEARCH_KEYS = [
  "agentSession",
  "agentSessionChannel",
  "autoSend",
  "channelManagement",
  "jump",
  "messageId",
  "profile",
  "profileTab",
  "profileView",
  "thread",
  "threadRootId",
] as const;

export type ChannelSearchKey = (typeof CHANNEL_SEARCH_KEYS)[number];

/**
 * Returns the search-param patch that clears only the `autoSend` trigger.
 *
 * Exported so tests can verify the patch is surgical — it must not include
 * `thread` or any other panel key that would collapse open panels. This is
 * the regression guard for the "auto-submit clear drops the thread route"
 * defect: a `goChannel()` re-navigation drops every search key including
 * `thread`; this patch removes only `autoSend` via `applyPatch`, preserving
 * the thread panel across the deferred `setTimeout(0)` submit.
 */
/**
 * Jump to a message the way message links do (see useChannelRouteTarget),
 * closing channel management in the same history entry. A main-timeline
 * message (no thread) is only scrolled to and highlighted (`jump`): its
 * reply panel stays closed.
 */
export function buildMessageJumpPatch(
  messageId: string,
  threadRootId: string | null,
): Partial<Record<ChannelSearchKey, string | null>> {
  return {
    channelManagement: null,
    jump: threadRootId ? null : "1",
    messageId,
    threadRootId,
  };
}

export function buildAutoSendClearPatch(): Partial<
  Record<ChannelSearchKey, string | null>
> {
  return { autoSend: null };
}
