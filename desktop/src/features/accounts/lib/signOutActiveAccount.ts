import * as React from "react";

import { accountLabel } from "@/features/accounts/lib/accountLabels";
import { beginAccountAction } from "@/features/accounts/lib/accountSwitch";
import {
  listAccounts,
  logoutOnThisDevice,
  logoutTokenSession,
  type TokenAuthStatus,
} from "@/shared/api/tokenAuth";

/**
 * Sign the active account out here. The backend stops its agents and, when
 * another account is still signed in, switches to it; either way the app
 * restarts once, under the next account or on the signed-out screen.
 */
export function signOutActiveAccount(
  onStatus: (status: TokenAuthStatus) => void = () => {},
): Promise<void> {
  return signOutWith(() => logoutTokenSession(), onStatus);
}

/**
 * The same sign-out on this computer only, for screens shown when the server
 * cannot be reached: nothing waits on the network, and ending the session on
 * the server is best effort in the background. Never the full local reset.
 */
export function signOutActiveAccountOnThisDevice(): Promise<void> {
  return signOutWith(logoutOnThisDevice, () => {});
}

/**
 * Whether the active account has a signed-in session here, so a sign-out
 * has something to do. Read from this computer only.
 */
export async function hasSignedInSession(): Promise<boolean> {
  const accounts = await listAccounts();
  return accounts.some(
    (account) => account.active && account.state === "signed_in",
  );
}

/**
 * [`hasSignedInSession`] for a screen: `false` until known (and when it
 * cannot be read), so a sign-out is only offered with something to sign out.
 */
export function useHasSignedInSession(): boolean {
  const [signedIn, setSignedIn] = React.useState(false);
  React.useEffect(() => {
    let live = true;
    hasSignedInSession()
      .then((value) => {
        if (live) setSignedIn(value);
      })
      .catch(() => {});
    return () => {
      live = false;
    };
  }, []);
  return signedIn;
}

async function signOutWith(
  logout: () => Promise<TokenAuthStatus>,
  onStatus: (status: TokenAuthStatus) => void,
): Promise<void> {
  const action = beginAccountAction();
  let next: TokenAuthStatus;
  try {
    next = await logout();
  } catch (error) {
    action.cancel();
    throw error;
  }
  onStatus(next);
  const notice = agentsNotStoppedNotice(next.agentsNotStopped ?? 0);
  const switchedTo = next.switchedTo;
  if (!switchedTo) {
    action.reload(null, notice);
    return;
  }
  const accounts = await listAccounts().catch(() => []);
  const to = accounts.find((account) => account.pubkey === switchedTo);
  action.reload(
    to ? `Switching to ${accountLabel(to)}…` : "Switching accounts…",
    notice,
  );
}

/** Said after a sign-out whose agents did not all confirm they stopped. */
function agentsNotStoppedNotice(count: number): string | null {
  if (count <= 0) return null;
  const agents = count === 1 ? "1 agent" : `${count} agents`;
  return `Signed out. ${agents} from that account did not stop in time; restart Kuzz if they keep replying.`;
}
