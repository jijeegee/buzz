import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import { closeSocketsThenReload } from "@/shared/api/relayWebSocketClose";
import { invokeTauri } from "@/shared/api/tauri";

/**
 * Google account bridge for the current community.
 *
 * Key-backup accounts keep their local Nostr signer through login and renewal.
 * Legacy token mode is per community: a relay that advertises `buzz_token_auth` in
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
  keyBackupSupported?: boolean;
  keyBackup?: boolean;
  legacyTokenAccount?: boolean;
  signingPubkey?: string | null;
  /**
   * Signed out on this computer: chats stay closed (and native relay auth is
   * refused) until an account signs in here. Nothing is deleted.
   */
  deviceSignedOut?: boolean;
  /**
   * The sign-in opened a different account than the one active before
   * (another Google account, or a protect whose Google account already held
   * an account). The app reloads into it.
   */
  accountChanged?: boolean;
  /** The account a sign-out of the active account switched to. */
  switchedTo?: string | null;
  /**
   * The chosen Google account has no Buzz account yet and nothing was
   * created: ask, then `confirmSignup` or `cancelSignup`.
   */
  signup?: SignupPrompt | null;
};

/** What the "Create your Buzz account" screen shows. */
export type SignupPrompt = {
  email: string | null;
  displayName: string | null;
  avatarUrl: string | null;
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

/**
 * Open the system browser for Google sign-in; resolves once signed in. The
 * chosen Google account decides the Buzz account, which becomes the active
 * account; accounts already signed in here stay signed in. `connect` links
 * the current key-only account to Google instead.
 */
export function loginWithGoogle(
  allowRestore = false,
  existingTokenAccount = false,
  attemptId?: string,
  connect = false,
): Promise<TokenAuthStatus> {
  return invokeTauri<TokenAuthStatus>("login_with_google", {
    allowRestore,
    ...(existingTokenAccount ? { existingTokenAccount: true } : {}),
    ...(attemptId ? { attemptId } : {}),
    ...(connect ? { connect: true } : {}),
  });
}

/** Tauri event emitted after the active account changed (payload: pubkey). */
export const ACTIVE_ACCOUNT_CHANGED_EVENT = "active-account-changed";

export function onActiveAccountChanged(
  handler: (pubkey: string) => void,
): Promise<UnlistenFn> {
  return listen<string>(ACTIVE_ACCOUNT_CHANGED_EVENT, (event) =>
    handler(event.payload),
  );
}

/** One Buzz account on this computer. */
export type AccountSummary = {
  pubkey: string;
  origin: string;
  state: "signed_in" | "needs_login" | "signed_out" | "key_only";
  active: boolean;
  /** The Google name. */
  displayName: string | null;
  /** The Buzz profile name; menus prefer it. */
  buzzName: string | null;
  avatarUrl: string | null;
  email: string | null;
  /** Unix seconds of the last sign-in or switch to it. */
  lastUsedAt: number | null;
};

/** A signed-in account watched in the background (not the open one). */
export type WatchableAccount = {
  origin: string;
  pubkey: string;
  displayName: string | null;
  buzzName: string | null;
  email: string | null;
};

/**
 * Signed-in accounts on every server except the open one, for background
 * notifications and unread badges. Re-read on `ACCOUNTS_CHANGED_EVENT`.
 */
export function listWatchableAccounts(): Promise<WatchableAccount[]> {
  return invokeTauri<WatchableAccount[]>("list_watchable_accounts");
}

/** Emitted when the accounts signed in on this computer may have changed. */
export const ACCOUNTS_CHANGED_EVENT = "accounts-changed";

/** Accounts on this computer for the current community, most recent first. */
export function listAccounts(): Promise<AccountSummary[]> {
  return invokeTauri<AccountSummary[]>("list_accounts");
}

/**
 * Record the active account's Buzz profile name for the account menu (ignored
 * unless `pubkey` is the active account).
 */
export function rememberAccountBuzzName(
  pubkey: string,
  name: string,
): Promise<void> {
  return invokeTauri<void>("remember_account_buzz_name", { pubkey, name });
}

/**
 * Make `pubkey` the active account; the others stay signed in. Resolves to a
 * warning when a server's session could not be handed over (the switch still
 * happened).
 */
export function switchAccount(pubkey: string): Promise<string | null> {
  return invokeTauri<string | null>("switch_account", { pubkey });
}

/** NIP-42 AUTH event signed by a signed-in account that may be inactive. */
export function createAuthEventAs(
  account: string,
  challenge: string,
  relayUrl: string,
): Promise<string> {
  return invokeTauri<string>("create_auth_event_as", {
    account,
    challenge,
    relayUrl,
  });
}

/** Decrypt a NIP-44 payload a signed-in (maybe inactive) account sent itself. */
export function nip44DecryptAs(
  account: string,
  ciphertext: string,
): Promise<string> {
  return invokeTauri<string>("nip44_decrypt_as", { account, ciphertext });
}

/** Cancel this attempt while waiting for the browser, without signing out. */
export function cancelGoogleLogin(attemptId: string): Promise<boolean> {
  return invokeTauri<boolean>("cancel_google_login", { attemptId });
}

/** Create the Buzz account the user confirmed and finish signing in. */
export function confirmSignup(): Promise<TokenAuthStatus> {
  return invokeTauri<TokenAuthStatus>("confirm_signup");
}

/** Drop a sign-in waiting for confirmation; nothing was created. */
export function cancelSignup(): Promise<void> {
  return invokeTauri<void>("cancel_signup");
}

/**
 * Sign `account` (default: the active account) out of the current community
 * on this computer: its session is revoked and its agents stop until it signs
 * in again. When the active account signs out and another one is still
 * signed in here, the backend switches to it (`switchedTo`).
 */
export function logoutTokenSession(account?: string): Promise<TokenAuthStatus> {
  return invokeTauri<TokenAuthStatus>("logout", account ? { account } : {});
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

export function renameAuthDevice(
  deviceId: string,
  name: string,
): Promise<AuthDevice> {
  return invokeTauri<AuthDevice>("rename_device", { deviceId, name });
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
  const identity = status.keyBackup ? status.signingPubkey : status.principal;
  return (
    status.state === "active" &&
    identity != null &&
    identity !== appliedPrincipal
  );
}

/**
 * Reloads into a new identity: every identity-scoped store restarts. An object
 * so tests can observe the call (jsdom cannot navigate).
 */
export const identityReload = {
  reload(): void {
    // Sockets first: a reload alone leaves them open in the native layer.
    void closeSocketsThenReload();
  },
};
