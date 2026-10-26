import * as React from "react";

/**
 * Downsized relay media for avatar slots.
 *
 * The relay serves JPEG variants next to every uploaded still image,
 * authorized by the same rules as the original:
 * - `/media/<sha256>.small.jpg` — 160px, for avatars up to 56 CSS px
 *   (rows, sidebar, chips);
 * - `/media/<sha256>.thumb.jpg` — 320px, for larger spots (headers, cards).
 *
 * Group photos and profile pictures are often multi-megabyte originals, so
 * avatars load a variant and fall back down the chain
 * (`small` → `thumb` → original) when one is missing (older uploads, a relay
 * without that variant, or a failed fetch).
 *
 * GIFs keep their original so animation is not lost, and URLs without a known
 * still-image extension are left alone.
 */
export type RelayMediaVariant = "small" | "thumb";

/** Largest CSS px size that uses the `small` variant. */
export const SMALL_MEDIA_MAX_CSS_PX = 56;

const RELAY_STILL_IMAGE_RE =
  /^(https?:\/\/[^/?#]+\/media\/[\da-f]{64})\.(?:jpg|png|webp)(?:[?#].*)?$/;

/** The variant for an avatar drawn at `cssPx` CSS pixels. */
export function relayMediaVariantForSize(cssPx: number): RelayMediaVariant {
  return cssPx <= SMALL_MEDIA_MAX_CSS_PX ? "small" : "thumb";
}

/**
 * The URLs to try, in order, for showing `url` at `variant` size: the
 * requested variant, any larger variant, then the original. Only the
 * original comes back when no variant applies (GIF, already a variant, other
 * paths, data URLs, animated-avatar URLs); empty input gives `[]`.
 */
export function relayMediaCandidates(
  url: string | null | undefined,
  variant: RelayMediaVariant,
): string[] {
  const trimmed = url?.trim();
  if (!trimmed) return [];
  if (trimmed.includes("#kuzz-anim=")) return [trimmed];
  const match = RELAY_STILL_IMAGE_RE.exec(trimmed);
  if (!match) return [trimmed];
  const base = match[1];
  return variant === "small"
    ? [`${base}.small.jpg`, `${base}.thumb.jpg`, trimmed]
    : [`${base}.thumb.jpg`, trimmed];
}

const MISSING_VARIANTS_KEY = "kuzz-missing-media-variants.v1";
const MISSING_VARIANT_TTL_MS = 7 * 24 * 60 * 60 * 1_000;
const MAX_MISSING_VARIANTS = 500;
let missingVariants: Map<string, number> | null = null;

/**
 * Variant URLs that failed to load, kept across launches for a week: an old
 * upload without variants otherwise cost every avatar slot two failed
 * requests before its original, again on every launch and every remount.
 */
function missingVariantMap(): Map<string, number> {
  if (missingVariants) return missingVariants;
  missingVariants = new Map();
  try {
    const raw = window.localStorage.getItem(MISSING_VARIANTS_KEY);
    const parsed: unknown = raw ? JSON.parse(raw) : null;
    if (parsed && typeof parsed === "object") {
      const now = Date.now();
      for (const [url, at] of Object.entries(parsed)) {
        if (typeof at === "number" && now - at < MISSING_VARIANT_TTL_MS) {
          missingVariants.set(url, at);
        }
      }
    }
  } catch {
    // Storage is a convenience: without it, variants are simply tried again.
  }
  return missingVariants;
}

/** Record that a variant URL could not be loaded. */
export function rememberMissingVariant(url: string): void {
  const map = missingVariantMap();
  map.delete(url);
  map.set(url, Date.now());
  while (map.size > MAX_MISSING_VARIANTS) {
    const oldest = map.keys().next().value;
    if (oldest === undefined) break;
    map.delete(oldest);
  }
  try {
    window.localStorage.setItem(
      MISSING_VARIANTS_KEY,
      JSON.stringify(Object.fromEntries(map)),
    );
  } catch {
    // Non-fatal: kept for this session only.
  }
}

/** `candidates` without variants known to be missing; the last is kept. */
export function withoutMissingVariants(candidates: string[]): string[] {
  if (candidates.length <= 1) return candidates;
  const map = missingVariantMap();
  const last = candidates[candidates.length - 1];
  return [
    ...candidates.slice(0, -1).filter((candidate) => !map.has(candidate)),
    last,
  ];
}

/**
 * Walk {@link relayMediaCandidates} as loads fail. Returns the raw (not
 * proxied) URL to load now plus an error handler; the handler returns `true`
 * when it moved on to another candidate, so callers only treat an error as
 * final once it returns `false`.
 */
export function useRelayMediaFallback(
  url: string | null | undefined,
  variant: RelayMediaVariant,
): {
  src: string | null;
  onError: () => boolean;
} {
  const candidates = withoutMissingVariants(relayMediaCandidates(url, variant));
  const key = candidates.join("\n");
  const [failed, setFailed] = React.useState({ key: "", count: 0 });
  const index = failed.key === key ? failed.count : 0;
  const hasNext = index + 1 < candidates.length;
  const current = candidates[index];
  const onError = React.useCallback(() => {
    if (!hasNext) return false;
    // A variant that failed is skipped from now on (see `missingVariantMap`).
    if (current) rememberMissingVariant(current);
    setFailed({ key, count: index + 1 });
    return true;
  }, [current, hasNext, index, key]);
  return { src: candidates[index] ?? null, onError };
}
