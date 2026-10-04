/**
 * Credential selection for relay HTTP requests and WebSocket AUTH.
 *
 * Token mode with a session → `Authorization: Bearer` / `["AUTH",{token}]`.
 * Otherwise the existing key path (NIP-98 / NIP-42 with NIP-07 or the
 * page-lifetime key), so flag-off relays and signed-out public reads behave
 * exactly as before. Flows that create durable membership pass
 * `requireSession` and fail with [`SignInRequiredError`] instead of falling
 * back to a throwaway key.
 */

import { SignInRequiredError } from "./web-session.ts";

/** Callbacks the selectors consult (real session + signer in production). */
export type AuthSources = {
  tokenMode: () => Promise<boolean>;
  accessToken: () => Promise<string | null>;
};

/** `Authorization` header value for a relay HTTP request. */
export async function resolveHttpAuthorization(
  sources: AuthSources & { nip98: () => Promise<string> },
  options?: { requireSession?: boolean },
): Promise<string> {
  if (await sources.tokenMode()) {
    const token = await sources.accessToken();
    if (token) return `Bearer ${token}`;
    if (options?.requireSession) throw new SignInRequiredError();
  }
  return sources.nip98();
}

/** OK id the relay answers a token AUTH frame with (plan §3.4). */
export const TOKEN_AUTH_OK_ID = "auth";

/** The frame answering a relay AUTH challenge and the OK id to wait for. */
export type WsAuthFrame = { okId: string; message: unknown[] };

/** Build the reply to a NIP-42 challenge. */
export async function buildWsAuthFrame(
  sources: AuthSources & {
    signChallenge: () => Promise<{ id: string }>;
  },
): Promise<WsAuthFrame> {
  if (await sources.tokenMode()) {
    const token = await sources.accessToken();
    if (token) {
      return { okId: TOKEN_AUTH_OK_ID, message: ["AUTH", { token }] };
    }
  }
  const signed = await sources.signChallenge();
  return { okId: signed.id, message: ["AUTH", signed] };
}
