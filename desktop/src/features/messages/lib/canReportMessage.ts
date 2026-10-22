import type { TimelineMessage } from "@/features/messages/types";
import { KIND_HUDDLE_STARTED } from "@/shared/constants/kinds";
import { normalizePubkey } from "@/shared/lib/pubkey";

/**
 * A message moderators can act on: a real, delivered event with a known
 * author. Pending sends and system huddle rows have neither.
 */
export function isModeratableMessage(message: TimelineMessage): boolean {
  return (
    !message.pending &&
    message.kind !== KIND_HUDDLE_STARTED &&
    Boolean(message.pubkey)
  );
}

/**
 * "Report message" is for other people's messages only. My own messages and
 * agent messages are never reportable, so the report dialog (and its "Also
 * block") can never target myself or an agent.
 */
export function canReportMessage({
  currentPubkey,
  isAgentPubkey,
  message,
}: {
  currentPubkey: string | undefined;
  isAgentPubkey: (pubkey: string) => boolean;
  message: TimelineMessage;
}): boolean {
  if (!isModeratableMessage(message) || !message.pubkey) return false;
  if (message.accent === true) return false;
  if (message.isAgent === true || message.role === "bot") return false;
  const author = normalizePubkey(message.pubkey);
  if (currentPubkey && author === normalizePubkey(currentPubkey)) return false;
  return !isAgentPubkey(author);
}
