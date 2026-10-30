import { requestIdentityReload } from "@/features/accounts/lib/accountSwitch";
import { getIdentity } from "@/shared/api/tauriIdentity";
import {
  getTokenAuthStatus,
  onActiveAccountChanged,
  onTokenAuthChanged,
  type TokenAuthStatus,
  tokenIdentityChanged,
} from "@/shared/api/tokenAuth";

/** The init run that owns the listeners: cancelled runs drop them. */
export type TokenAuthWatchScope = {
  isCancelled: () => boolean;
  keep: (unlisten: () => void) => void;
};

function own(scope: TokenAuthWatchScope, listening: Promise<() => void>) {
  void listening.then((unlisten) => {
    if (scope.isCancelled()) unlisten();
    else scope.keep(unlisten);
  });
}

/**
 * Another account became active (a switch from another window, or a session
 * ending here). It is announced even when the new account's session cannot
 * be restored right now (offline), which a status check would not see.
 */
export function watchActiveAccount(
  appliedAccount: string | null,
  scope: TokenAuthWatchScope,
): void {
  own(
    scope,
    onActiveAccountChanged((pubkey) => {
      if (!scope.isCancelled() && pubkey !== appliedAccount) {
        requestIdentityReload();
      }
    }),
  );
}

/**
 * If the account identity changes after init (a restore that was still
 * retrying completes, or another path signs in), this computer signs in or
 * out, or the session ends, every identity-scoped store restarts under it,
 * as sign-in does.
 */
export function watchTokenStatus(
  status: TokenAuthStatus,
  identityPubkey: string | null,
  scope: TokenAuthWatchScope,
): void {
  const deviceSignedOut = status.deviceSignedOut === true;
  const appliedPrincipal =
    status.keyBackup || status.keyBackupSupported
      ? identityPubkey
      : status.state === "active"
        ? status.principal
        : null;
  own(
    scope,
    onTokenAuthChanged(() => {
      if (scope.isCancelled()) return;
      void getTokenAuthStatus()
        .then((next) => {
          if (
            !scope.isCancelled() &&
            (tokenIdentityChanged(appliedPrincipal, next) ||
              (next.deviceSignedOut === true) !== deviceSignedOut ||
              // The session ended (any code): the regular launch shows it.
              // That launch starts from needs_login, so it never loops.
              (next.state === "needs_login" && status.state !== "needs_login"))
          ) {
            requestIdentityReload();
          }
        })
        .catch(() => {});
    }),
  );
}

/**
 * Whether a restore that finished after the stored account was drawn leaves
 * that account in place. Anything else (signed out here, the session ended,
 * another identity) reloads into the regular launch, which waits for the
 * restore and shows the signed-out screen or the new account.
 */
export function restoreKeepsDrawnAccount(
  status: TokenAuthStatus,
  drawnPubkey: string,
  identityPubkey: string,
): boolean {
  return (
    status.deviceSignedOut !== true &&
    status.state !== "needs_login" &&
    identityPubkey === drawnPubkey &&
    !tokenIdentityChanged(drawnPubkey, status)
  );
}

/**
 * The session restore behind a launch that drew the stored account at once
 * (docs/message-sync.md#launch-order). `release` opens the relay socket; it
 * is called only once the restore has settled (signed in, ended, or no
 * session). A restore still retrying when the status call gives up keeps
 * the socket held until a token-auth change reports it settled.
 */
export async function restoreDrawnAccount(
  restore: Promise<TokenAuthStatus>,
  drawnPubkey: string,
  release: () => void,
  scope: TokenAuthWatchScope,
): Promise<void> {
  let status: TokenAuthStatus;
  try {
    status = await restore;
  } catch (error) {
    // As the regular launch: the drawn account stays, without the watcher.
    console.error("[useCommunityInit] token auth status failed:", error);
    release();
    return;
  }
  if (scope.isCancelled()) return;
  if (status.state !== "restoring") {
    await settleDrawnAccount(status, drawnPubkey, release, scope);
    return;
  }
  let settled = false;
  own(
    scope,
    onTokenAuthChanged(() => {
      if (scope.isCancelled() || settled) return;
      void getTokenAuthStatus()
        .then((next) => {
          if (scope.isCancelled() || settled || next.state === "restoring") {
            return;
          }
          settled = true;
          return settleDrawnAccount(next, drawnPubkey, release, scope);
        })
        .catch(() => {});
    }),
  );
}

async function settleDrawnAccount(
  status: TokenAuthStatus,
  drawnPubkey: string,
  release: () => void,
  scope: TokenAuthWatchScope,
): Promise<void> {
  release();
  let identityPubkey = drawnPubkey;
  if (status.state === "active") {
    try {
      identityPubkey = (await getIdentity()).pubkey;
    } catch (error) {
      console.error("[useCommunityInit] identity read failed:", error);
    }
  }
  if (scope.isCancelled()) return;
  if (!restoreKeepsDrawnAccount(status, drawnPubkey, identityPubkey)) {
    requestIdentityReload();
    return;
  }
  if (status.supported) watchTokenStatus(status, drawnPubkey, scope);
}
