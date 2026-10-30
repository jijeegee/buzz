/**
 * Where a room visit opens, decided on the rows it holds, stored or fetched
 * (docs/message-sync.md#opening-at-a-position): a linked message, the first
 * unread message, where the reader left, the newest message. Rows that are
 * not a current window (a stored paint, a head cache) decide too, but only
 * provisionally: once the relay's window is shown the same rule runs again,
 * and the open moves if the reader has not scrolled. Shared cases:
 * test-fixtures/message-store-paint-first.json.
 */

export type OpenAnchorInput = {
  /** A link, a huddle transcript or a jump to the newest decides instead. */
  opensElsewhere: boolean;
  /** The read marker taken when the visit opened; null: never read. */
  frontierSeconds: number | null;
  /** The marker was taken without the read state: trust no position. */
  frontierUnknown: boolean;
  /** First unread top-level row among the loaded ones. */
  firstUnreadMessageId: string | null;
  /** Where the reader left the room last time. */
  remembered: { messageId: string; topPx: number | null } | null;
  /** Whether a row is loaded. */
  hasRow: (messageId: string) => boolean;
  /** Time of the oldest loaded top-level row; null when none is loaded. */
  oldestTopLevelAt: number | null;
  hasOlderMessages: boolean;
  /** Another older page may still be asked for. */
  canSeekOlder: boolean;
  /** The seek ran out of time. */
  seekExpired: boolean;
};

export type OpenAnchorDecision =
  | {
      kind: "open";
      anchorMessageId: string | null;
      anchorTopPx: number | null;
      /** A fallback to the newest that keeps the remembered row. */
      keepsRemembered: boolean;
    }
  | { kind: "seek" };

const SEEK: OpenAnchorDecision = { kind: "seek" };

function open(
  anchorMessageId: string | null,
  anchorTopPx: number | null = null,
  keepsRemembered = false,
): OpenAnchorDecision {
  return {
    kind: "open",
    anchorMessageId,
    anchorTopPx: anchorMessageId === null ? null : anchorTopPx,
    keepsRemembered,
  };
}

export function decideOpenAnchor(input: OpenAnchorInput): OpenAnchorDecision {
  if (input.opensElsewhere) return open(null);
  const frontier = input.frontierSeconds;
  if (frontier === null || input.firstUnreadMessageId === null) {
    // Nothing unread: go back to where the reader left, if it is close.
    const remembered = input.frontierUnknown ? null : input.remembered;
    if (remembered === null) return open(null, null, input.frontierUnknown);
    if (input.hasRow(remembered.messageId)) {
      return open(remembered.messageId, remembered.topPx);
    }
    if (input.canSeekOlder) return SEEK;
    // Out of time keeps the row; out of pages forgets it.
    return open(null, null, input.seekExpired);
  }
  // The rows reach back to the read marker unless every one is unread and
  // older ones exist: then the first unread may be older (ask the relay).
  const everyLoadedRowUnread =
    input.oldestTopLevelAt !== null && input.oldestTopLevelAt > frontier;
  if (!everyLoadedRowUnread || !input.hasOlderMessages) {
    return open(input.firstUnreadMessageId);
  }
  // Unread for too long: the start is too far back to open at.
  return input.canSeekOlder ? SEEK : open(null);
}

/**
 * What a provisional open does once the relay's window is shown, or
 * `undefined` to stay: the reader scrolled, or the rule agrees. Otherwise it
 * moves to the new row, or seeks older pages first when the row may be older
 * than the loaded ones.
 */
export function reanchorTarget(
  provisionalAnchorId: string | null,
  onCurrent: OpenAnchorDecision,
  readerMoved: boolean,
): OpenAnchorDecision | undefined {
  if (readerMoved) return undefined;
  if (onCurrent.kind === "seek") return onCurrent;
  return onCurrent.anchorMessageId === provisionalAnchorId
    ? undefined
    : onCurrent;
}

/**
 * Whether stored rows may decide where a room opens. `completeFrom` is how
 * far back the store holds the room's events without a gap (`undefined`: the
 * rows are not a store paint; `null`: unknown). A first unread row only
 * counts when every event since the read marker is held; otherwise an
 * earlier unread one may be missing, so the open waits for the relay.
 */
export function roomPaintDecides(
  completeFrom: number | null | undefined,
  firstUnreadMessageId: string | null,
  frontierSeconds: number | null,
): boolean {
  if (completeFrom === undefined || firstUnreadMessageId === null) return true;
  if (frontierSeconds === null) return true;
  return completeFrom !== null && completeFrom <= frontierSeconds;
}

/**
 * The same for a thread's stored replies: the reply before the first
 * unread one (the newest read one) must be at or after `completeFrom`, so
 * nothing between them is missing. A first unread reply that is the oldest
 * loaded one is decided by older pages anyway when there are any; without
 * them only a whole stored history counts.
 */
export function threadPaintDecides(
  completeFrom: number | null | undefined,
  firstUnreadAt: number | null,
  readBeforeAt: number | null,
  hasOlderReplies: boolean,
): boolean {
  if (completeFrom === undefined || firstUnreadAt === null) return true;
  if (completeFrom === null) return false;
  if (readBeforeAt !== null) return completeFrom <= readBeforeAt;
  return hasOlderReplies || completeFrom === 0;
}
