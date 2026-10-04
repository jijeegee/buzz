import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import { invokeTauri } from "@/shared/api/tauri";

/**
 * Centralized-identity (Google sign-in) bridge for the current community.
 *
 * Token mode is per community: a relay that advertises `buzz_token_auth` in
 * NIP-11 offers "Sign in with Google"; once signed in, the Rust side signs
 * and authenticates as the account principal (bearer tokens, server-stamped
 * drafts) instead of the local key. Every call acts on the active community.
 */

export type TokenAuthState =
  | "signed_out"
  | "restoring"
  | "active"
  | "needs_login";

export type TokenAuthStatus = {
  origin: string;
  supported: boolean;
  providers: string[];
  state: TokenAuthState;
  principal: string | null;
  deviceId: string | null;
  reason: string | null;
};

export type AuthDevice = {
  id: string;
  name: string;
  platform: string;
  lastSeenAt: string | null;
  current: boolean;
};

/** Tauri event emitted when a community's sign-in state changes. */
export const TOKEN_AUTH_CHANGED_EVENT = "token-auth-changed";

/** Report (restoring a stored session on first call) the current state. */
export function getTokenAuthStatus(): Promise<TokenAuthStatus> {
  return invokeTauri<TokenAuthStatus>("get_token_auth_status");
}

/** Open the system browser for Google sign-in; resolves once signed in. */
export function loginWithGoogle(): Promise<TokenAuthStatus> {
  return invokeTauri<TokenAuthStatus>("login_with_google");
}

/** Sign out of the current community (revokes this device's session). */
export function logoutTokenSession(): Promise<TokenAuthStatus> {
  return invokeTauri<TokenAuthStatus>("logout");
}

/**
 * The WebSocket AUTH payload for the current community: `{token}` in token
 * mode, `null` in key mode (the caller signs a NIP-42 event instead).
 */
export function getWsAuthFrame(): Promise<{ token: string } | null> {
  return invokeTauri<{ token: string } | null>("get_ws_auth_frame");
}

export function listAuthDevices(): Promise<AuthDevice[]> {
  return invokeTauri<AuthDevice[]>("list_devices");
}

export function revokeAuthDevice(deviceId: string): Promise<void> {
  return invokeTauri<void>("revoke_device", { deviceId });
}

export function revokeOtherSessions(): Promise<void> {
  return invokeTauri<void>("revoke_other_sessions");
}

export function revokeAllBotTokens(): Promise<void> {
  return invokeTauri<void>("revoke_all_bot_tokens");
}

export function deleteTokenAccount(): Promise<TokenAuthStatus> {
  return invokeTauri<TokenAuthStatus>("delete_account");
}

/** Agents re-registered under a new bot identity since the last call. */
export function takeTokenAuthNotices(): Promise<string[]> {
  return invokeTauri<string[]>("take_token_auth_notices");
}

export function onTokenAuthChanged(
  handler: (origin: string) => void,
): Promise<UnlistenFn> {
  return listen<string>(TOKEN_AUTH_CHANGED_EVENT, (event) =>
    handler(event.payload),
  );
}

/**
 * Whether the community's signed-in account now differs from the one the app
 * initialised with (`appliedPrincipal`, `null` when it started without one).
 */
export function tokenIdentityChanged(
  appliedPrincipal: string | null,
  status: TokenAuthStatus,
): boolean {
  return status.state === "active" && status.principal !== appliedPrincipal;
}

/**
 * Reloads into a new identity: every identity-scoped store restarts. An object
 * so tests can observe the call (jsdom cannot navigate).
 */
export const identityReload = {
  reload(): void {
    window.location.reload();
  },
};
