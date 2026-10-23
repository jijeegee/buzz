import type { RelayEvent } from "@/shared/api/types";

/** Relay-signed excerpt of a message sent before the reader joined. */
export const KIND_QUOTE_PREVIEW = 39008;

export type QuotePreviewRequest = {
  channelId: string;
  quotedId: string;
  quotingId: string;
};

export type QuotePreviewFallback =
  | {
      kind: "ready";
      /** Original author from the preview's `p` tag. */
      authorPubkey: string;
      /** Author name looked up for this preview; null when none resolved. */
      authorName: string | null;
      excerpt: string;
    }
  | { kind: "none" };

type ParsedQuotePreview = { authorPubkey: string; excerpt: string };

function tagValue(event: RelayEvent, name: string): string | undefined {
  return event.tags.find((tag) => tag[0] === name)?.[1];
}

/**
 * Validate a relay quote preview against what was asked: kind:39008 for the
 * quoted id (`e`/`d`), in this channel (`h`), naming an author (`p`), with a
 * JSON body carrying a non-empty `excerpt`. Anything else is no preview.
 */
export function parseQuotePreviewEvent(
  event: RelayEvent | null | undefined,
  channelId: string,
  quotedId: string,
): ParsedQuotePreview | null {
  if (!event || event.kind !== KIND_QUOTE_PREVIEW) return null;
  const quoted = tagValue(event, "e") ?? tagValue(event, "d");
  if (quoted?.toLowerCase() !== quotedId.toLowerCase()) return null;
  if (tagValue(event, "h") !== channelId) return null;
  const authorPubkey = tagValue(event, "p")?.trim().toLowerCase();
  if (!authorPubkey) return null;
  let body: unknown;
  try {
    body = JSON.parse(event.content);
  } catch {
    return null;
  }
  const excerpt =
    body && typeof body === "object"
      ? (body as { excerpt?: unknown }).excerpt
      : undefined;
  if (typeof excerpt !== "string" || !excerpt.trim()) return null;
  return { authorPubkey, excerpt: excerpt.trim() };
}

export type QuotePreviewDeps = {
  fetchPreview: (request: QuotePreviewRequest) => Promise<RelayEvent | null>;
  fetchAuthorName: (pubkey: string) => Promise<string | null>;
};

const cache = new Map<string, Promise<QuotePreviewFallback>>();

export function resetQuotePreviewCache() {
  cache.clear();
}

export function quotePreviewCacheKey(request: QuotePreviewRequest): string {
  return `${request.channelId}:${request.quotedId}:${request.quotingId}`;
}

/**
 * Fetch (once per channel, quoted and quoting message) the preview shown when
 * the quoted message itself cannot be fetched. A failed request is not cached,
 * so a later render retries; a definitive "no preview" is.
 */
export function fetchQuotePreviewFallback(
  request: QuotePreviewRequest,
  deps: QuotePreviewDeps,
): Promise<QuotePreviewFallback> {
  const key = quotePreviewCacheKey(request);
  const cached = cache.get(key);
  if (cached) return cached;

  let failed = false;
  const pending = deps
    .fetchPreview(request)
    .then(async (event): Promise<QuotePreviewFallback> => {
      const parsed = parseQuotePreviewEvent(
        event,
        request.channelId,
        request.quotedId,
      );
      if (!parsed) return { kind: "none" };
      const authorName = await deps
        .fetchAuthorName(parsed.authorPubkey)
        .catch(() => null);
      return { kind: "ready", ...parsed, authorName };
    })
    .catch((): QuotePreviewFallback => {
      failed = true;
      return { kind: "none" };
    });
  cache.set(key, pending);
  void pending.then(() => {
    if (failed && cache.get(key) === pending) cache.delete(key);
  });
  return pending;
}
