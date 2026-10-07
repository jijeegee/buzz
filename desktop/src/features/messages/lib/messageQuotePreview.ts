import { summarizeMessageLinkContent } from "@/features/messages/lib/messageLinkMetadata";
import type { QuoteReference } from "@/features/messages/lib/messageQuote";
import { getThreadReference } from "@/features/messages/lib/threading";
import {
  resolveUserLabel,
  type UserProfileLookup,
} from "@/features/profile/lib/identity";
import type { RelayEvent } from "@/shared/api/types";

export type QuotedPreview =
  | { kind: "loading" }
  | { kind: "unavailable" }
  | {
      kind: "ready";
      author: string;
      excerpt: string;
      /** Thread root of the quoted message; null when it is top-level. */
      threadRootId: string | null;
    };

/** Find the quoted event among already-loaded timeline and thread caches. */
export function findLoadedQuotedEvent(
  eventId: string,
  caches: readonly (readonly RelayEvent[] | undefined)[],
): RelayEvent | null {
  for (const events of caches) {
    const match = events?.find((event) => event.id === eventId);
    if (match) return match;
  }
  return null;
}

/**
 * Build the quoted-message header from a loaded event. The quoting author
 * recorded the quoted author in the `q` tag; prefer it for the label so a
 * delegated (relay-signed) message still names its real author.
 */
export function quotedPreviewFromEvent(
  reference: QuoteReference,
  event: RelayEvent,
  profiles?: UserProfileLookup,
): QuotedPreview {
  return {
    kind: "ready",
    author: resolveUserLabel({
      pubkey: reference.authorPubkey ?? event.pubkey,
      preferResolvedSelfLabel: true,
      profiles,
    }),
    excerpt: summarizeMessageLinkContent(event.content),
    threadRootId: getThreadReference(event.tags).rootId,
  };
}
