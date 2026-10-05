import { getThreadReference } from "@/features/messages/lib/threading";
import type { RouterRecentMessage } from "@/shared/api/tauriMessageRouting";
import type { RelayEvent } from "@/shared/api/types";
import {
  KIND_STREAM_MESSAGE,
  KIND_STREAM_MESSAGE_V2,
} from "@/shared/constants/kinds";
import { normalizePubkey } from "@/shared/lib/pubkey";

/** Recent context for Smart routing: at most this many messages… */
export const ROUTER_RECENT_MAX = 20;
/** …from at most this far before the routed message… */
export const ROUTER_RECENT_WINDOW_SECONDS = 3 * 60 * 60;
/** …each cut to this many characters (Rust re-enforces both caps). */
export const ROUTER_RECENT_MAX_CHARS = 300;

/**
 * The messages before `message` in its own conversation — the same thread
 * for a reply, the top level otherwise — from already-cached channel
 * events, oldest first. No relay fetch: whatever is cached is the context.
 */
export function selectRouterRecent(
  events: readonly RelayEvent[],
  message: RelayEvent,
  ownerPubkey: string | null,
): RouterRecentMessage[] {
  const threadRoot = getThreadReference(message.tags).rootId;
  const owner = ownerPubkey ? normalizePubkey(ownerPubkey) : null;
  const since = message.created_at - ROUTER_RECENT_WINDOW_SECONDS;
  return events
    .filter((event) => {
      if (
        event.id === message.id ||
        (event.kind !== KIND_STREAM_MESSAGE &&
          event.kind !== KIND_STREAM_MESSAGE_V2) ||
        event.created_at < since ||
        event.created_at > message.created_at
      ) {
        return false;
      }
      const root = getThreadReference(event.tags).rootId;
      // A reply's thread root already rides as THREAD ROOT.
      return root === threadRoot;
    })
    .sort((a, b) => a.created_at - b.created_at)
    .slice(-ROUTER_RECENT_MAX)
    .map((event) => ({
      pubkey: event.pubkey,
      isOwner: owner !== null && normalizePubkey(event.pubkey) === owner,
      content: event.content.slice(0, ROUTER_RECENT_MAX_CHARS),
      createdAt: event.created_at,
    }));
}
