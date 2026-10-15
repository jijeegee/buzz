import { accountLabel } from "@/features/accounts/lib/accountLabels";
import { beginAccountAction } from "@/features/accounts/lib/accountSwitch";
import {
  listAccounts,
  logoutTokenSession,
  type TokenAuthStatus,
} from "@/shared/api/tokenAuth";

/**
 * Sign the active account out here. The backend stops its agents and, when
 * another account is still signed in, switches to it; either way the app
 * restarts once, under the next account or on the signed-out screen.
 */
export async function signOutActiveAccount(
  onStatus: (status: TokenAuthStatus) => void = () => {},
): Promise<void> {
  const action = beginAccountAction();
  let next: TokenAuthStatus;
  try {
    next = await logoutTokenSession();
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
