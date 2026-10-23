import type { RelayEvent } from "@/shared/api/types";
import { KIND_NEW_SIGNIN_NOTICE } from "@/shared/constants/kinds";

const PREFIX = "New sign-in: ";
/** Longest device name shown; the relay caps names, this guards the layout. */
const MAX_NAME = 64;

export type NewSignInNotice = {
  id: string;
  deviceName: string;
};

/**
 * The relay's "new sign-in" notice for `me` (kind 24211, `p` = the account,
 * content `New sign-in: <device name>`), or `null` for anything else.
 */
export function parseNewSignInNotice(
  event: RelayEvent,
  me: string,
): NewSignInNotice | null {
  if (event.kind !== KIND_NEW_SIGNIN_NOTICE) return null;
  const forMe = event.tags.some(
    (tag) => tag[0] === "p" && tag[1]?.toLowerCase() === me.toLowerCase(),
  );
  if (!forMe || !event.content.startsWith(PREFIX)) return null;
  const deviceName = event.content
    .slice(PREFIX.length)
    .replace(/\p{Cc}/gu, "")
    .trim()
    .slice(0, MAX_NAME);
  return deviceName ? { id: event.id, deviceName } : null;
}
