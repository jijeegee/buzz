/**
 * Per-server, per-account localStorage record of where the reader left each
 * channel's main timeline (and each thread, keyed `channelId/rootId`): the
 * message at the top of the view and its distance from the scroller top. A
 * channel left at its newest message has no entry, so it opens at the newest
 * message again.
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

/** Before positions were kept per server: one record per account. */
const LEGACY_STORAGE_PREFIX = "kuzz-reading-position.v1";
const STORAGE_PREFIX = "kuzz-reading-position.v2";
export const MAX_READING_POSITION_ENTRIES = 300;
const storageKey = (scope: string) => `${STORAGE_PREFIX}:${scope}`;

/**
 * `https://host[:port]` for a relay URL, the same server identity the device
 * message store files use (src-tauri message_store `http_origin`), so a socket
 * and a query to one server share a record.
 */
function httpOrigin(relayUrl: string): string | null {
  try {
    const url = new URL(relayUrl.trim());
    const scheme =
      url.protocol === "wss:" || url.protocol === "https:"
        ? "https"
        : url.protocol === "ws:" || url.protocol === "http:"
          ? "http"
          : null;
    if (scheme === null || !url.hostname) return null;
    return `${scheme}://${url.host.toLowerCase()}`;
  } catch {
    return null;
  }
}

/**
 * The record a reader's positions live in: one per server and account. Null
 * (nothing is kept) without both.
 */
export function readingPositionScope(
  pubkey: string | null | undefined,
  relayUrl: string | null | undefined,
): string | null {
  const origin = relayUrl ? httpOrigin(relayUrl) : null;
  if (!pubkey || origin === null) return null;
  return `${origin}|${pubkey}`;
}

/**
 * Moves a pre-server record of the scope's account into the scope the first
 * time that account reads on any server, then drops the old record.
 */
function migrateLegacy(scope: string) {
  const legacyKey = `${LEGACY_STORAGE_PREFIX}:${scope.slice(scope.lastIndexOf("|") + 1)}`;
  const legacy = window.localStorage.getItem(legacyKey);
  if (legacy === null) return;
  if (window.localStorage.getItem(storageKey(scope)) === null) {
    window.localStorage.setItem(storageKey(scope), legacy);
  }
  window.localStorage.removeItem(legacyKey);
}

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

function readMap(scope: string): ReadingPositionMap {
  try {
    migrateLegacy(scope);
    const raw = window.localStorage.getItem(storageKey(scope));
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

function writeMap(scope: string, map: ReadingPositionMap) {
  try {
    const entries = Object.entries(map);
    if (entries.length === 0) {
      window.localStorage.removeItem(storageKey(scope));
      return;
    }
    window.localStorage.setItem(
      storageKey(scope),
      JSON.stringify(
        Object.fromEntries(entries.slice(-MAX_READING_POSITION_ENTRIES)),
      ),
    );
  } catch {
    // Best-effort: a full or unavailable store just forgets positions.
  }
}

export const readingPositionStore = {
  /** `scope` comes from readingPositionScope. */
  get(scope: string, channelId: string): ReadingPosition | null {
    return readMap(scope)[channelId] ?? null;
  },

  /** Records `position` for `channelId`, or forgets it when null. */
  set(scope: string, channelId: string, position: ReadingPosition | null) {
    const map = readMap(scope);
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
    writeMap(scope, map);
  },
};
