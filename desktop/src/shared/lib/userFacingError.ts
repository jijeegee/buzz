/**
 * Kuzz users sign in with Google and never manage keys or pick a relay, so
 * error text that leaks protocol vocabulary (relay, Nostr, npub/nsec, NIP-xx,
 * raw socket URLs) means nothing to them. Raw messages stay intact everywhere
 * else because code matches on them; only the display layer maps them.
 */
const DEVELOPER_TERM_PATTERN =
  /\b(?:relays?|nostr|npub1?[0-9a-z]*|nsec1?[0-9a-z]*|nip-\d+|pubkeys?|public keys?|private keys?)\b|\bwss?:\/\//i;

const CONNECTIVITY_PATTERN =
  /\b(?:unreachable|\w*connect\w*|timed out|timeouts?|network|offline|websockets?|reach\w*)\b/i;

export const CONNECTIVITY_ERROR_MESSAGE =
  "Couldn't connect. Check your internet connection and try again.";
export const GENERIC_ERROR_MESSAGE = "Something went wrong. Try again.";

export function containsDeveloperTerm(message: string): boolean {
  return DEVELOPER_TERM_PATTERN.test(message);
}

export function toUserFacingMessage(message: string): string {
  if (!containsDeveloperTerm(message)) {
    return message;
  }
  return CONNECTIVITY_PATTERN.test(message)
    ? CONNECTIVITY_ERROR_MESSAGE
    : GENERIC_ERROR_MESSAGE;
}
