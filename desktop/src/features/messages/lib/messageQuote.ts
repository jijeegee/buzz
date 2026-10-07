import { summarizeMessageLinkContent } from "@/features/messages/lib/messageLinkMetadata";
import type { TimelineMessage } from "@/features/messages/types";
import { KIND_HUDDLE_STARTED } from "@/shared/constants/kinds";

/**
 * NIP-18 quote tag name: `["q", <event id>, <relay url or "">, <author pubkey>]`.
 *
 * A quote is a reference only. It never decides where a message lands: the
 * `e` reply tags alone place a message on the main timeline or in a thread.
 */
export const QUOTE_TAG = "q";

const HEX_64 = /^[0-9a-f]{64}$/;

/** What a composer shows (and sends) while the user is quoting a message. */
export type QuoteTarget = {
  /** Display name of the quoted author, for the composer chip. */
  author: string;
  /** Hex pubkey of the quoted author, carried as the `q` tag's 4th value. */
  authorPubkey: string;
  /** Hex event id of the quoted message. */
  eventId: string;
  /** One-line plain-text excerpt of the quoted message. */
  excerpt: string;
};

export type QuoteReference = {
  authorPubkey: string | null;
  eventId: string;
  relayUrl: string | null;
};

function normalizeHex(value: string | null | undefined): string | null {
  const normalized = value?.trim().toLowerCase() ?? "";
  return HEX_64.test(normalized) ? normalized : null;
}

/**
 * Build the outgoing NIP-18 quote tag set for a quoted event. Returns a list
 * so it can be merged into the composer's outgoing tag set alongside imeta,
 * emoji, and mention tags (see `splitOutgoingTags`).
 */
export function buildQuoteTags(
  eventId: string,
  authorPubkey: string,
  relayUrl = "",
): string[][] {
  const normalizedEventId = normalizeHex(eventId);
  if (!normalizedEventId) {
    throw new Error("A quoted message needs a valid event ID.");
  }
  const normalizedAuthor = normalizeHex(authorPubkey);
  if (!normalizedAuthor) {
    throw new Error("A quoted message needs a valid author pubkey.");
  }
  return [[QUOTE_TAG, normalizedEventId, relayUrl.trim(), normalizedAuthor]];
}

/** The first well-formed `q` tag on a message, if any. */
export function getQuoteReference(
  tags: readonly (readonly string[])[] | null | undefined,
): QuoteReference | null {
  for (const tag of tags ?? []) {
    if (tag[0] !== QUOTE_TAG) continue;
    const eventId = normalizeHex(tag[1]);
    if (!eventId) continue;
    return {
      authorPubkey: normalizeHex(tag[3]),
      eventId,
      relayUrl: tag[2]?.trim() || null,
    };
  }
  return null;
}

/** Delivered, ordinary messages with a known author can be quoted. */
export function canQuoteMessage(message: TimelineMessage): boolean {
  return (
    !message.pending &&
    message.kind !== KIND_HUDDLE_STARTED &&
    normalizeHex(message.id) !== null &&
    normalizeHex(message.pubkey ?? message.signerPubkey) !== null
  );
}

export function quoteTargetFromMessage(
  message: TimelineMessage,
): QuoteTarget | null {
  if (!canQuoteMessage(message)) return null;
  const eventId = normalizeHex(message.id);
  const authorPubkey = normalizeHex(message.pubkey ?? message.signerPubkey);
  if (!eventId || !authorPubkey) return null;
  return {
    author: message.author,
    authorPubkey,
    eventId,
    excerpt: summarizeMessageLinkContent(message.body),
  };
}

/** Outgoing tag set for an optional quote (empty when nothing is quoted). */
export function quoteTargetTags(target: QuoteTarget | null): string[][] {
  return target ? buildQuoteTags(target.eventId, target.authorPubkey) : [];
}

/**
 * Merge a composer's quote into its outgoing tag set. Without a quote the
 * caller's tags pass through untouched (including `undefined`).
 */
export function withQuoteTags(
  mediaTags: string[][] | undefined,
  target: QuoteTarget | null,
): string[][] | undefined {
  if (!target) return mediaTags;
  return [
    ...(mediaTags ?? []).filter((tag) => tag[0] !== QUOTE_TAG),
    ...quoteTargetTags(target),
  ];
}
