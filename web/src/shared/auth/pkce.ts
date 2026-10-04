/**
 * PKCE + state for the web Google sign-in (plan §3.2, web row).
 *
 * The verifier and state live in `sessionStorage` between the redirect to
 * `/auth/oidc/google/start` and the return to `/auth/cb`. They are consumed
 * exactly once: a callback whose `state` does not match is rejected and the
 * pending login is discarded, so its code can never be completed.
 */

/** Storage key for the pending login. */
export const PENDING_LOGIN_KEY = "buzz.web.oidc.pending";

/** The minimal `Storage` surface used here (sessionStorage in the browser). */
export type KeyValueStorage = Pick<
  Storage,
  "getItem" | "setItem" | "removeItem"
>;

/** A login started in this tab and not yet completed. */
export type PendingLogin = {
  state: string;
  verifier: string;
  /** Same-origin path to return to after sign-in. */
  returnTo: string;
};

/** The callback `state` does not match the login this tab started. */
export class LoginStateMismatchError extends Error {
  constructor() {
    super("Sign-in response did not match this browser tab. Try again.");
    this.name = "LoginStateMismatchError";
  }
}

function base64Url(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary)
    .replace(/\+/g, "-")
    .replace(/\//g, "_")
    .replace(/=+$/, "");
}

function randomUrlSafe(byteLength: number): string {
  const bytes = new Uint8Array(byteLength);
  crypto.getRandomValues(bytes);
  return base64Url(bytes);
}

/** S256 challenge for `verifier` (43 URL-safe characters). */
export async function pkceChallenge(verifier: string): Promise<string> {
  const digest = await crypto.subtle.digest(
    "SHA-256",
    new TextEncoder().encode(verifier),
  );
  return base64Url(new Uint8Array(digest));
}

/** Only same-origin absolute paths are allowed as a post-login destination. */
export function safeReturnTo(path: string | null | undefined): string {
  // Exactly one leading "/" (a path on this origin), no backslash (browsers
  // treat "/\host" like "//host"), no control characters.
  if (
    typeof path !== "string" ||
    !path.startsWith("/") ||
    path.startsWith("//")
  ) {
    return "/";
  }
  for (const char of path) {
    const code = char.charCodeAt(0);
    if (char === "\\" || code < 0x20 || code === 0x7f) return "/";
  }
  let url: URL;
  try {
    url = new URL(path, RETURN_TO_BASE);
  } catch {
    return "/";
  }
  if (url.origin !== RETURN_TO_BASE) return "/";
  if (url.pathname === "/auth" || url.pathname.startsWith("/auth/")) return "/";
  return `${url.pathname}${url.search}${url.hash}`;
}

/** Placeholder origin used only to normalize a same-origin return path. */
const RETURN_TO_BASE = "https://return-to.invalid";

/**
 * Create and store a pending login; returns the relay `start` URL to
 * navigate to.
 */
export async function beginLogin(options: {
  storage: KeyValueStorage;
  /** HTTP base URL of the relay serving `/auth/*`. */
  relayBaseUrl: string;
  /** Origin of this web app; the relay redirects to `<origin>/auth/cb`. */
  redirectOrigin: string;
  provider?: string;
  returnTo?: string;
  deviceName?: string;
}): Promise<string> {
  const pending: PendingLogin = {
    state: randomUrlSafe(24),
    verifier: randomUrlSafe(32),
    returnTo: safeReturnTo(options.returnTo),
  };
  options.storage.setItem(PENDING_LOGIN_KEY, JSON.stringify(pending));
  const relayBase = options.relayBaseUrl.replace(/\/+$/, "");
  const redirectOrigin = options.redirectOrigin.replace(/\/+$/, "");
  const query = new URLSearchParams({
    state: pending.state,
    code_challenge: await pkceChallenge(pending.verifier),
    client: "web",
    redirect_uri: `${redirectOrigin}/auth/cb`,
    device_name: options.deviceName ?? "Web browser",
  });
  const provider = encodeURIComponent(options.provider ?? "google");
  return `${relayBase}/auth/oidc/${provider}/start?${query.toString()}`;
}

/**
 * Take (and always remove) the pending login for a callback carrying
 * `returnedState`. Throws [`LoginStateMismatchError`] when there is no
 * pending login or the state differs.
 */
export function takePendingLogin(
  storage: KeyValueStorage,
  returnedState: string | null,
): PendingLogin {
  const raw = storage.getItem(PENDING_LOGIN_KEY);
  storage.removeItem(PENDING_LOGIN_KEY);
  let pending: PendingLogin | null = null;
  try {
    pending = raw ? (JSON.parse(raw) as PendingLogin) : null;
  } catch {
    pending = null;
  }
  if (
    !pending ||
    typeof pending.state !== "string" ||
    typeof pending.verifier !== "string" ||
    !returnedState ||
    pending.state !== returnedState
  ) {
    throw new LoginStateMismatchError();
  }
  return { ...pending, returnTo: safeReturnTo(pending.returnTo) };
}
