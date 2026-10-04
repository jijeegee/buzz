/**
 * Browser session for relays that advertise token auth (plan §3.2 web row,
 * §2.3): the access token lives only in this object's memory; the refresh
 * token is the relay's HttpOnly `buzz_refresh` cookie, which JavaScript never
 * sees. `POST /auth/refresh` with an empty body rotates the cookie and
 * returns a new access token.
 *
 * - Refresh is single-flight: concurrent callers share one request, so one
 *   page never rotates the cookie twice at once.
 * - Every result is fenced by a generation counter: a refresh that completes
 *   after sign-out or a new sign-in is discarded instead of resurrecting the
 *   old session (AGENTS.md rule 2).
 * - Transient failures back off and stop after a bounded number of attempts
 *   (rule 4); a terminal 401/400 signs the page out.
 */

/** Session state shown to the UI. */
export type SessionStatus =
  | "unknown"
  | "signed_out"
  | "signed_in"
  | "unavailable";

/** Immutable snapshot for `useSyncExternalStore`. */
export type SessionSnapshot = {
  status: SessionStatus;
  principalId: string | null;
  displayName: string | null;
};

type FetchLike = (input: string, init?: RequestInit) => Promise<Response>;

/** Injected platform dependencies (real browser APIs in production). */
export type WebSessionDeps = {
  /** HTTP base URL of the relay (`https://host`). */
  baseUrl: string;
  fetch: FetchLike;
  now?: () => number;
  setTimer?: (callback: () => void, ms: number) => unknown;
  clearTimer?: (handle: unknown) => void;
};

/** A request needs a signed-in session and there is none. */
export class SignInRequiredError extends Error {
  constructor() {
    super("Sign in with Google to continue.");
    this.name = "SignInRequiredError";
  }
}

/** Refresh this long before the access token expires. */
export const REFRESH_LEAD_MS = 5 * 60_000;
/** An access token closer than this to expiry is not handed out. */
const MIN_REMAINING_MS = 60_000;
/** Transient-failure backoff schedule; after the last step the session stops retrying. */
export const RETRY_DELAYS_MS = [5_000, 15_000, 60_000, 120_000, 300_000];

type AccessToken = { token: string; expiresAt: number };

type RefreshOutcome =
  | { kind: "ok"; access: AccessToken }
  | { kind: "terminal" }
  | { kind: "transient" };

export class WebSession {
  private readonly baseUrl: string;
  private readonly fetchImpl: FetchLike;
  private readonly now: () => number;
  private readonly setTimer: (callback: () => void, ms: number) => unknown;
  private readonly clearTimer: (handle: unknown) => void;

  private access: AccessToken | null = null;
  private generation = 0;
  private inflight: Promise<string | null> | null = null;
  private timer: unknown = null;
  private failures = 0;
  private snapshot: SessionSnapshot = {
    status: "unknown",
    principalId: null,
    displayName: null,
  };
  private readonly listeners = new Set<() => void>();

  constructor(deps: WebSessionDeps) {
    this.baseUrl = deps.baseUrl.replace(/\/+$/, "");
    this.fetchImpl = deps.fetch;
    this.now = deps.now ?? Date.now;
    this.setTimer = deps.setTimer ?? ((cb, ms) => setTimeout(cb, ms));
    this.clearTimer =
      deps.clearTimer ??
      ((handle) => clearTimeout(handle as ReturnType<typeof setTimeout>));
  }

  /** Subscribe to snapshot changes; returns the unsubscribe function. */
  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  /** Current snapshot (stable identity until the next change). */
  getSnapshot = (): SessionSnapshot => this.snapshot;

  /**
   * A usable access token, refreshing via the cookie when the cached one is
   * missing or near expiry. `null` when signed out.
   */
  async getAccessToken(): Promise<string | null> {
    if (this.access && this.access.expiresAt - this.now() > MIN_REMAINING_MS) {
      return this.access.token;
    }
    if (this.snapshot.status === "signed_out") return null;
    return this.refresh();
  }

  /** Restore a session from the refresh cookie (call once at boot). */
  restore(): Promise<string | null> {
    return this.refresh();
  }

  /** Rotate the refresh cookie now. Concurrent callers share one request. */
  refresh(): Promise<string | null> {
    if (!this.inflight) {
      const generation = this.generation;
      this.inflight = this.runRefresh(generation).finally(() => {
        this.inflight = null;
      });
    }
    return this.inflight;
  }

  /** Complete a Google sign-in started by `beginLogin`. */
  async completeLogin(loginCode: string, codeVerifier: string): Promise<void> {
    const response = await this.fetchImpl(
      `${this.baseUrl}/auth/oidc/complete`,
      {
        method: "POST",
        credentials: "include",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          login_code: loginCode,
          code_verifier: codeVerifier,
        }),
      },
    );
    const json = (await response.json().catch(() => ({}))) as Record<
      string,
      unknown
    >;
    if (!response.ok || typeof json.access !== "string") {
      throw new Error(
        typeof json.error === "string" ? json.error : `HTTP ${response.status}`,
      );
    }
    this.generation += 1;
    this.failures = 0;
    this.setAccess(json.access, json.expires_in);
    this.update({
      status: "signed_in",
      principalId:
        typeof json.principal_id === "string" ? json.principal_id : null,
      displayName: null,
    });
  }

  /**
   * Revoke this browser's session on the relay (which also clears the
   * cookie), then forget it locally. A failure propagates and keeps the
   * session: forgetting it locally while the cookie still works would sign
   * the user back in on the next page load.
   */
  async signOut(): Promise<void> {
    const token = await this.getAccessToken();
    if (token) {
      let response: Response;
      try {
        response = await this.fetchImpl(`${this.baseUrl}/auth/logout`, {
          method: "POST",
          credentials: "include",
          headers: { Authorization: `Bearer ${token}` },
        });
      } catch {
        throw new Error("Could not reach the relay to sign out. Try again.");
      }
      // 401: the session is already dead on the relay.
      if (!response.ok && response.status !== 401) {
        throw new Error(`Sign-out failed (HTTP ${response.status}).`);
      }
    } else if (this.snapshot.status !== "signed_out") {
      throw new Error("Could not reach the relay to sign out. Try again.");
    }
    this.forget();
  }

  private profileInflight: Promise<void> | null = null;

  /** Load the profile (`GET /auth/me`) into the snapshot. Best effort. */
  loadProfile(): Promise<void> {
    if (!this.profileInflight) {
      this.profileInflight = this.fetchProfile().finally(() => {
        this.profileInflight = null;
      });
    }
    return this.profileInflight;
  }

  private async fetchProfile(): Promise<void> {
    const generation = this.generation;
    const token = await this.getAccessToken();
    if (!token) return;
    try {
      const response = await this.fetchImpl(`${this.baseUrl}/auth/me`, {
        headers: { Authorization: `Bearer ${token}` },
      });
      if (!response.ok || generation !== this.generation) return;
      const me = (await response.json()) as Record<string, unknown>;
      this.update({
        ...this.snapshot,
        principalId:
          typeof me.principal_id === "string"
            ? me.principal_id
            : this.snapshot.principalId,
        displayName:
          typeof me.display_name === "string" && me.display_name
            ? me.display_name
            : null,
      });
    } catch {
      // The name is cosmetic; the session itself is unaffected.
    }
  }

  private async runRefresh(generation: number): Promise<string | null> {
    const outcome = await this.requestRefresh();
    if (generation !== this.generation) {
      // Signed out or signed in again meanwhile: this result is stale.
      return this.access?.token ?? null;
    }
    switch (outcome.kind) {
      case "ok":
        this.failures = 0;
        this.access = outcome.access;
        this.scheduleRefresh(outcome.access.expiresAt - REFRESH_LEAD_MS);
        if (this.snapshot.status !== "signed_in") {
          this.update({ ...this.snapshot, status: "signed_in" });
        }
        return outcome.access.token;
      case "terminal":
        this.forget();
        return null;
      case "transient": {
        const delay = RETRY_DELAYS_MS[this.failures];
        this.failures += 1;
        if (delay !== undefined) this.scheduleRefresh(this.now() + delay, 0);
        const usable =
          this.access && this.access.expiresAt > this.now()
            ? this.access
            : null;
        if (!usable) {
          this.access = null;
          this.update({ ...this.snapshot, status: "unavailable" });
        }
        return usable?.token ?? null;
      }
    }
  }

  private async requestRefresh(): Promise<RefreshOutcome> {
    let response: Response;
    try {
      response = await this.fetchImpl(`${this.baseUrl}/auth/refresh`, {
        method: "POST",
        credentials: "include",
        headers: { "Content-Type": "application/json" },
        body: "{}",
      });
    } catch {
      return { kind: "transient" };
    }
    // 400: no refresh cookie. 401: invalid, expired, revoked, reused, disabled.
    if (response.status === 400 || response.status === 401) {
      return { kind: "terminal" };
    }
    if (!response.ok) return { kind: "transient" };
    const json = (await response.json().catch(() => ({}))) as Record<
      string,
      unknown
    >;
    if (typeof json.access !== "string") return { kind: "transient" };
    return {
      kind: "ok",
      access: {
        token: json.access,
        expiresAt: this.now() + expiresInMs(json.expires_in),
      },
    };
  }

  private setAccess(token: string, expiresIn: unknown) {
    this.access = { token, expiresAt: this.now() + expiresInMs(expiresIn) };
    this.scheduleRefresh(this.access.expiresAt - REFRESH_LEAD_MS);
  }

  /**
   * Arm the single refresh timer for `at`, no sooner than `minDelayMs` (so a
   * short-lived token cannot drive a tight refresh loop).
   */
  private scheduleRefresh(at: number, minDelayMs = 30_000) {
    if (this.timer !== null) this.clearTimer(this.timer);
    const delay = Math.max(at - this.now(), minDelayMs);
    this.timer = this.setTimer(() => {
      this.timer = null;
      void this.refresh();
    }, delay);
  }

  private forget() {
    this.generation += 1;
    this.access = null;
    this.failures = 0;
    if (this.timer !== null) {
      this.clearTimer(this.timer);
      this.timer = null;
    }
    this.update({ status: "signed_out", principalId: null, displayName: null });
  }

  private update(next: SessionSnapshot) {
    this.snapshot = next;
    for (const listener of this.listeners) listener();
  }
}

function expiresInMs(value: unknown): number {
  const seconds = typeof value === "number" && value > 0 ? value : 3600;
  return seconds * 1000;
}
