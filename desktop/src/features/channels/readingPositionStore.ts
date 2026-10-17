/**
 * Per-pubkey localStorage record of where the reader left each channel's main
 * timeline: the message at the top of the view and its distance from the
 * scroller top. A channel left at its newest message has no entry, so it opens
 * at the newest message again.
 *
 * Device-local and best-effort; never synced to the relay.
 */

export type ReadingPosition = {
  messageId: string;
  /** Distance (px) of the message's row from the top of the scroll view. */
  topPx: number;
};

/** channelId → where the reader left that channel. */
type ReadingPositionMap = Record<string, ReadingPosition>;

const STORAGE_PREFIX = "kuzz-reading-position.v1";
export const MAX_READING_POSITION_ENTRIES = 300;
const storageKey = (pubkey: string) => `${STORAGE_PREFIX}:${pubkey}`;

function isReadingPosition(value: unknown): value is ReadingPosition {
  return (
    typeof value === "object" &&
    value !== null &&
    "messageId" in value &&
    typeof value.messageId === "string" &&
    value.messageId.length > 0 &&
    "topPx" in value &&
    typeof value.topPx === "number" &&
    Number.isFinite(value.topPx)
  );
}

function readMap(pubkey: string): ReadingPositionMap {
  try {
    const raw = window.localStorage.getItem(storageKey(pubkey));
    if (!raw) return {};
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed))
      return {};
    const result: ReadingPositionMap = {};
    for (const [channelId, position] of Object.entries(parsed)) {
      if (isReadingPosition(position)) {
        result[channelId] = {
          messageId: position.messageId,
          topPx: position.topPx,
        };
      }
    }
    return result;
  } catch {
    return {};
  }
}

function writeMap(pubkey: string, map: ReadingPositionMap) {
  try {
    const entries = Object.entries(map);
    if (entries.length === 0) {
      window.localStorage.removeItem(storageKey(pubkey));
      return;
    }
    window.localStorage.setItem(
      storageKey(pubkey),
      JSON.stringify(
        Object.fromEntries(entries.slice(-MAX_READING_POSITION_ENTRIES)),
      ),
    );
  } catch {
    // Best-effort: a full or unavailable store just forgets positions.
  }
}

export const readingPositionStore = {
  get(pubkey: string, channelId: string): ReadingPosition | null {
    return readMap(pubkey)[channelId] ?? null;
  },

  /** Records `position` for `channelId`, or forgets it when null. */
  set(pubkey: string, channelId: string, position: ReadingPosition | null) {
    const map = readMap(pubkey);
    const current = map[channelId];
    if (position === null) {
      if (current === undefined) return;
      delete map[channelId];
    } else {
      const topPx = Math.round(position.topPx);
      if (current?.messageId === position.messageId && current.topPx === topPx)
        return;
      // Re-insert so the most recently read channels survive the bound.
      delete map[channelId];
      map[channelId] = { messageId: position.messageId, topPx };
    }
    writeMap(pubkey, map);
  },
};
