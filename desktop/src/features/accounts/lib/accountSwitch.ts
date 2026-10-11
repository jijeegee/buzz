import { identityReload } from "@/shared/api/tokenAuth";

/**
 * Reloads into another account, and the one-reload overlay that bridges it.
 *
 * Every account change (switch, add, sign-out that lands on another account)
 * restarts the app under the new identity with a reload. The backend also
 * announces the change (`active-account-changed`, `token-auth-changed`), and
 * the community init listener reloads on those for changes made elsewhere.
 * An action started in this window holds those reloads until it settles, so
 * it can write the overlay caption and any notice first, then reloads once.
 */

const SWITCHING_KEY = "buzz:switching-to";
const NOTICE_KEY = "buzz:account-switch-notice";

let holds = 0;
let deferred = false;

/** Reload into the new identity, unless an account action here holds it. */
export function requestIdentityReload(): void {
  if (holds > 0) {
    deferred = true;
    return;
  }
  identityReload.reload();
}

export type AccountAction = {
  /** Reload into the new account, showing `caption` while the app restarts. */
  reload(caption: string | null, notice?: string | null): void;
  /** Nothing changed here; a reload asked for meanwhile still happens. */
  cancel(): void;
};

/** Hold identity reloads for one account action until it settles. */
export function beginAccountAction(): AccountAction {
  holds += 1;
  let settled = false;
  const release = (): boolean => {
    if (settled) return false;
    settled = true;
    holds -= 1;
    if (holds > 0) return false;
    const wanted = deferred;
    deferred = false;
    return wanted;
  };
  return {
    reload(caption, notice) {
      release();
      try {
        if (caption) window.sessionStorage.setItem(SWITCHING_KEY, caption);
        if (notice) window.sessionStorage.setItem(NOTICE_KEY, notice);
      } catch {
        // Only the overlay caption is lost.
      }
      identityReload.reload();
    },
    cancel() {
      if (release()) identityReload.reload();
    },
  };
}

let bootCaption: string | null = null;
let bootNotice: string | null = null;

/**
 * Read and clear what the previous page left for this load. Called once at
 * startup, unconditionally, so a caption never outlives the reload it was
 * written for.
 */
export function takeAccountSwitchAtBoot(): void {
  try {
    bootCaption = window.sessionStorage.getItem(SWITCHING_KEY);
    bootNotice = window.sessionStorage.getItem(NOTICE_KEY);
    window.sessionStorage.removeItem(SWITCHING_KEY);
    window.sessionStorage.removeItem(NOTICE_KEY);
  } catch {
    bootCaption = null;
    bootNotice = null;
  }
}

/** "Switching to …" while the app starts after an account change. */
export function accountSwitchCaption(): string | null {
  return bootCaption;
}

/**
 * The app is up under the new account: the caption is done, and any notice
 * from the change is handed out once.
 */
export function finishAccountSwitchBoot(): string | null {
  const notice = bootNotice;
  bootCaption = null;
  bootNotice = null;
  return notice;
}

/** Test seam: forget held reloads and boot state. */
export function resetAccountSwitchForTests(): void {
  holds = 0;
  deferred = false;
  bootCaption = null;
  bootNotice = null;
}
