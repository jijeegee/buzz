import { createAuthEvent } from "@/shared/api/tauri";
import { getWsAuthFrame } from "@/shared/api/tokenAuth";

/**
 * The relay labels its OK reply to a token AUTH frame with this id (there is
 * no event id: the frame carries a bearer token, not a signed event).
 */
export const TOKEN_AUTH_OK_ID = "auth";

/** What to send for a relay AUTH challenge, and which OK id answers it. */
export type RelayAuthFrame = { pendingId: string; payload: unknown };

type AuthFrameDeps = {
  tokenFrame: () => Promise<{ token: string } | null>;
  signChallenge: (input: {
    challenge: string;
    relayUrl: string;
  }) => Promise<{ id: string }>;
};

const defaultDeps: AuthFrameDeps = {
  tokenFrame: getWsAuthFrame,
  signChallenge: createAuthEvent,
};

/**
 * Answer a relay `["AUTH", challenge]`: a community signed in with Google
 * sends `["AUTH", {"token"}]` (OK id `auth`); otherwise the NIP-42 kind:22242
 * event signed by the local key (OK id = event id). The token path never
 * signs, so a token-mode community can never authenticate as the local key.
 */
export async function buildRelayAuthFrame(
  input: { challenge: string; relayUrl: string },
  deps: AuthFrameDeps = defaultDeps,
): Promise<RelayAuthFrame> {
  const token = await deps.tokenFrame();
  if (token) {
    return { pendingId: TOKEN_AUTH_OK_ID, payload: token };
  }
  const event = await deps.signChallenge(input);
  return { pendingId: event.id, payload: event };
}
