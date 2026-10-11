import * as React from "react";
import { useQuery } from "@tanstack/react-query";
import { toast } from "sonner";

import {
  accountAfterSignOut,
  accountEmailDetail,
  accountLabel,
} from "@/features/accounts/lib/accountLabels";
import { beginAccountAction } from "@/features/accounts/lib/accountSwitch";
import {
  type AuthDevice,
  deleteTokenAccount,
  getTokenAuthStatus,
  listAccounts,
  listAuthDevices,
  logoutTokenSession,
  onTokenAuthChanged,
  revokeAllBotTokens,
  revokeAuthDevice,
  revokeOtherSessions,
  takeTokenAuthNotices,
  type TokenAuthStatus,
} from "@/shared/api/tokenAuth";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/shared/ui/alert-dialog";
import {
  OLDER_ACCOUNT_SIGN_IN_LABEL,
  OLDER_ACCOUNT_SIGN_IN_TITLE,
} from "@/features/onboarding/ui/GoogleSignInButton";
import { listOwnRunningLocalAgents } from "@/features/agents/lib/runningLocalAgents";
import { useGoogleLogin } from "@/shared/hooks/useGoogleLogin";
import { GoogleLoginProgress } from "@/shared/ui/GoogleLoginProgress";
import { Button } from "@/shared/ui/button";
import { needsGoogleProtection } from "@/shared/lib/signingKeyUi";
import { Spinner } from "@/shared/ui/spinner";
import { AccountDeviceRow } from "./AccountDeviceRow";
import { SettingsConfirmDialog } from "./SettingsConfirmDialog";
import { SettingsOptionGroup, SettingsOptionRow } from "./SettingsOptionGroup";

function errorText(error: unknown, fallback: string): string {
  return error instanceof Error
    ? error.message
    : typeof error === "string"
      ? error
      : fallback;
}

/**
 * A signed-in identity change re-initialises every community-scoped store;
 * reloading is the one path that guarantees nothing keyed to the previous
 * identity survives (AGENTS.md "Community Switching"). After sign-out it lands
 * on the signed-out screen.
 */
function reloadIntoNewIdentity() {
  window.location.reload();
}

export const SIGN_OUT_DESCRIPTION =
  "You'll be signed out on this computer. Sign in with Google to get back to your conversations. Nothing is deleted.";

/** The sign-out text when another account here takes over. */
export function signOutDescription(nextAccount: string | null): string {
  return nextAccount
    ? `You'll be signed out on this computer and switched to ${nextAccount}. Nothing is deleted.`
    : SIGN_OUT_DESCRIPTION;
}

/** "1 agent of this account will stop." / "N agents …". */
export function agentsWillStopLabel(count: number): string {
  return count === 1
    ? "1 agent of this account will stop."
    : `${count} agents of this account will stop.`;
}

/**
 * Sign the active account out here. The backend stops its agents and, when
 * another account is still signed in, switches to it; either way the app
 * restarts once, under the next account or on the signed-out screen.
 */
async function signOutActiveAccount(
  onStatus: (status: TokenAuthStatus) => void,
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
  const switchedTo = next.switchedTo;
  if (!switchedTo) {
    action.reload(null);
    return;
  }
  const accounts = await listAccounts().catch(() => []);
  const to = accounts.find((account) => account.pubkey === switchedTo);
  action.reload(
    to ? `Switching to ${accountLabel(to)}…` : "Switching accounts…",
  );
}

/**
 * Settings › Profile › Account: Google sign-in for the active community and,
 * once signed in, its devices and account-level security actions. Renders
 * nothing on a relay that does not offer token sign-in. `onStatusChange`
 * lets the Profile panel adapt to whether Google holds the account.
 */
export function AccountSettingsCard({
  onStatusChange,
}: {
  onStatusChange?: (status: TokenAuthStatus | null) => void;
} = {}) {
  const login = useGoogleLogin();
  const [status, setStatus] = React.useState<TokenAuthStatus | null>(null);
  const [busy, setBusy] = React.useState<string | null>(null);

  React.useEffect(() => {
    // Only settled statuses: the Profile panel may remount this card to move
    // it, and a fresh mount's initial null must not bounce it back.
    if (status) onStatusChange?.(status);
  }, [onStatusChange, status]);

  const refresh = React.useCallback(async () => {
    try {
      setStatus(await getTokenAuthStatus());
    } catch (error) {
      console.error("[account] token auth status failed:", error);
    }
  }, []);

  React.useEffect(() => {
    void refresh();
    let unlisten: (() => void) | undefined;
    let disposed = false;
    void onTokenAuthChanged(() => void refresh()).then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    });
    void takeTokenAuthNotices()
      .then((agents) => {
        if (agents.length > 0) {
          toast.info(
            agents.length === 1
              ? "An agent got a new identity on this community. Its earlier messages stay under its old identity."
              : `${agents.length} agents got new identities on this community. Their earlier messages stay under their old identities.`,
          );
        }
      })
      .catch(() => {});
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [refresh]);

  async function run(label: string, action: () => Promise<unknown>) {
    setBusy(label);
    try {
      await action();
    } catch (error) {
      toast.error(errorText(error, `${label} failed.`));
    } finally {
      setBusy(null);
    }
  }

  if (!status?.supported) return null;

  const signIn = (existingTokenAccount = false) =>
    run("Sign in", async () => {
      // A local-only account links its own key; otherwise the chosen Google
      // account decides which account opens.
      const connect = !existingTokenAccount && needsGoogleProtection(status);
      const next = await login.run(false, existingTokenAccount, connect);
      if (!next) return;
      setStatus(next);
      if (
        !next.keyBackup ||
        next.accountChanged ||
        next.signingPubkey !== status.signingPubkey
      )
        reloadIntoNewIdentity();
    });

  const signOut = () => run("Sign out", () => signOutActiveAccount(setStatus));

  // Not yet held by Google: invite the user to protect the account with it.
  const protect = needsGoogleProtection(status);

  return (
    <div className={protect ? "mb-8" : "mt-12"} data-testid="settings-account">
      <SettingsOptionGroup
        description={
          protect
            ? "Continue with Google so you can sign in to Buzz on other computers and get back into your account if this one is lost. Your Buzz server keeps a protected copy for this."
            : "You sign in to Buzz with your Google account."
        }
        title={protect ? "Protect your account" : "Account"}
      >
        {status.state === "active" ? (
          <ActiveAccount
            busy={busy}
            onStatus={setStatus}
            run={run}
            status={status}
          />
        ) : (
          <SettingsOptionRow>
            <div className="min-w-0" aria-live="polite">
              <p className="font-medium">
                {status.state === "restoring"
                  ? "Restoring your sign-in…"
                  : status.state === "needs_login"
                    ? "Your sign-in has ended"
                    : protect
                      ? "Not connected to Google yet"
                      : "Not signed in"}
              </p>
              <p className="text-sm text-muted-foreground/70">
                {status.state === "needs_login"
                  ? status.keyBackup
                    ? "Sign in with Google again to manage your account. Buzz and your agents keep working."
                    : "Sign in with Google again to keep using this community and its agents."
                  : protect
                    ? "Your browser opens to finish. Your profile, communities and agents stay the same."
                    : "Sign in with Google to use your account on this community. Your browser opens to finish signing in."}
              </p>
            </div>
            <Button
              data-testid="account-sign-in"
              disabled={busy !== null || status.state === "restoring"}
              onClick={() => void signIn()}
              type="button"
            >
              {busy === "Sign in" ? (
                <Spinner aria-hidden className="h-4 w-4 border-2" />
              ) : null}
              {busy === "Sign in"
                ? "Signing in…"
                : status.state === "needs_login"
                  ? "Sign in again"
                  : protect
                    ? "Continue with Google"
                    : "Sign in with Google"}
            </Button>
            {status.keyBackupSupported &&
            !status.keyBackup &&
            !status.legacyTokenAccount &&
            status.state === "signed_out" ? (
              <Button
                data-testid="existing-token-sign-in"
                disabled={busy !== null}
                onClick={() => void signIn(true)}
                type="button"
                variant="outline"
                title={OLDER_ACCOUNT_SIGN_IN_TITLE}
              >
                {OLDER_ACCOUNT_SIGN_IN_LABEL}
              </Button>
            ) : null}
            {status.state === "restoring" ? (
              // Rule 6: a restore that keeps failing must not strand the user;
              // signing out (forgetting the stored session) stays reachable.
              <Button
                data-testid="account-sign-out"
                disabled={busy !== null}
                onClick={() => void signOut()}
                type="button"
                variant="outline"
              >
                Sign out
              </Button>
            ) : null}
          </SettingsOptionRow>
        )}
        <GoogleLoginProgress login={login} />
      </SettingsOptionGroup>
    </div>
  );
}

function ActiveAccount({
  status,
  busy,
  run,
  onStatus,
}: {
  status: TokenAuthStatus;
  busy: string | null;
  run: (label: string, action: () => Promise<unknown>) => Promise<void>;
  onStatus: (status: TokenAuthStatus) => void;
}) {
  const [devices, setDevices] = React.useState<AuthDevice[] | null>(null);
  const [confirmDelete, setConfirmDelete] = React.useState(false);
  const [confirming, setConfirming] = React.useState<
    "sign-out" | "sign-out-others" | "disconnect-agents" | null
  >(null);
  // Counted fresh each time the sign-out dialog opens.
  const [runningCount, setRunningCount] = React.useState(0);
  const openSignOut = () => {
    setRunningCount(0);
    setConfirming("sign-out");
    void listOwnRunningLocalAgents().then((running) =>
      setRunningCount(running.pubkeys.length),
    );
  };
  const accountsQuery = useQuery({
    queryKey: ["accounts"],
    queryFn: listAccounts,
  });
  const active = accountsQuery.data?.find((account) => account.active);
  const name = active ? accountLabel(active) : null;
  const email = active ? accountEmailDetail(active) : null;
  const next = accountAfterSignOut(accountsQuery.data ?? []);
  const closeConfirm = (open: boolean) => {
    if (!open) setConfirming(null);
  };

  const loadDevices = React.useCallback(async () => {
    try {
      setDevices(await listAuthDevices());
    } catch (error) {
      toast.error(errorText(error, "Could not load devices."));
    }
  }, []);

  React.useEffect(() => {
    void loadDevices();
  }, [loadDevices]);

  const disabled = busy !== null;

  return (
    <>
      <SettingsOptionRow>
        <div className="min-w-0">
          <p className="truncate font-medium" data-testid="account-name">
            {name ?? "Signed in with Google"}
          </p>
          {name ? (
            <p className="truncate text-sm text-muted-foreground/70">
              {email ?? "Signed in with Google"}
            </p>
          ) : null}
          <p className="text-sm text-muted-foreground/70">
            Switch or add accounts from your profile menu.
          </p>
        </div>
        <Button
          data-testid="account-sign-out"
          disabled={disabled}
          onClick={openSignOut}
          type="button"
          variant="outline"
        >
          Sign out
        </Button>
      </SettingsOptionRow>
      <SettingsConfirmDialog
        confirmLabel="Sign out"
        description={signOutDescription(next ? accountLabel(next) : null)}
        onConfirm={() =>
          void run("Sign out", () => signOutActiveAccount(onStatus))
        }
        onOpenChange={closeConfirm}
        open={confirming === "sign-out"}
        testId="account-sign-out-dialog"
        title={name ? `Sign out of ${name}?` : "Sign out of Buzz?"}
      >
        {runningCount > 0 ? (
          <p
            className="text-sm text-muted-foreground"
            data-testid="account-sign-out-agents"
          >
            {agentsWillStopLabel(runningCount)}
          </p>
        ) : null}
      </SettingsConfirmDialog>
      <SettingsOptionRow className="items-start">
        <div className="min-w-0 flex-1">
          <p className="font-medium" id="account-devices-heading">
            Devices
          </p>
          {devices === null ? (
            <p className="text-sm text-muted-foreground/70">Loading…</p>
          ) : (
            <ul
              aria-labelledby="account-devices-heading"
              className="mt-2 space-y-2"
            >
              {devices.map((device) => (
                <AccountDeviceRow
                  device={device}
                  disabled={disabled}
                  key={device.id}
                  onChanged={loadDevices}
                  onSignOut={() =>
                    void run("Sign out device", async () => {
                      await revokeAuthDevice(device.id);
                      await loadDevices();
                    })
                  }
                  run={run}
                />
              ))}
            </ul>
          )}
        </div>
      </SettingsOptionRow>
      <SettingsOptionRow>
        <p className="min-w-0 text-sm text-muted-foreground/70">
          Sign out every other device. Your agents keep running.
        </p>
        <Button
          data-testid="account-sign-out-others"
          disabled={disabled}
          onClick={() => setConfirming("sign-out-others")}
          type="button"
          variant="outline"
        >
          Sign out other devices
        </Button>
      </SettingsOptionRow>
      <SettingsConfirmDialog
        confirmLabel="Sign out other devices"
        description="Every other computer and phone using your account is signed out and will need to sign in with Google again. This computer stays signed in, and your agents keep running."
        onConfirm={() =>
          void run("Sign out other devices", async () => {
            await revokeOtherSessions();
            await loadDevices();
            toast.success("Signed out of all other devices.");
          })
        }
        onOpenChange={closeConfirm}
        open={confirming === "sign-out-others"}
        testId="account-sign-out-others-dialog"
        title="Sign out other devices?"
      />
      {!status.keyBackup ? (
        <>
          <SettingsOptionRow>
            <p className="min-w-0 text-sm text-muted-foreground/70">
              Disconnect every agent right away. Agents on this computer
              reconnect the next time they start.
            </p>
            <Button
              data-testid="account-disconnect-agents"
              disabled={disabled}
              onClick={() => setConfirming("disconnect-agents")}
              type="button"
              variant="outline"
            >
              Disconnect all agents
            </Button>
          </SettingsOptionRow>
          <SettingsConfirmDialog
            confirmLabel="Disconnect agents"
            description="All of your agents are disconnected from this community right away. Agents on this computer reconnect on their own the next time they start."
            destructive
            onConfirm={() =>
              void run("Disconnect agents", async () => {
                await revokeAllBotTokens();
                toast.success("All agents were disconnected.");
              })
            }
            onOpenChange={closeConfirm}
            open={confirming === "disconnect-agents"}
            testId="account-disconnect-agents-dialog"
            title="Disconnect all agents?"
          />
          <details className="px-4 py-3">
            <summary className="cursor-pointer text-sm text-muted-foreground">
              Account deletion
            </summary>
            <SettingsOptionRow>
              <p className="min-w-0 text-sm text-muted-foreground/70">
                Delete your account on this community. Signing in again within
                30 days restores it.
              </p>
              <Button
                disabled={disabled}
                onClick={() => setConfirmDelete(true)}
                type="button"
                variant="destructive"
              >
                Delete account
              </Button>
            </SettingsOptionRow>
          </details>
          <AlertDialog onOpenChange={setConfirmDelete} open={confirmDelete}>
            <AlertDialogContent>
              <AlertDialogHeader>
                <AlertDialogTitle>Delete your account?</AlertDialogTitle>
                <AlertDialogDescription>
                  Every device and agent is signed out now. The account is
                  removed after 30 days unless you sign in with Google again
                  before then.
                </AlertDialogDescription>
              </AlertDialogHeader>
              <AlertDialogFooter>
                <AlertDialogCancel disabled={disabled}>
                  Cancel
                </AlertDialogCancel>
                <Button
                  disabled={disabled}
                  onClick={() =>
                    void run("Delete account", async () => {
                      await deleteTokenAccount();
                      window.location.reload();
                    })
                  }
                  type="button"
                  variant="destructive"
                >
                  Delete account
                </Button>
              </AlertDialogFooter>
            </AlertDialogContent>
          </AlertDialog>
        </>
      ) : null}
    </>
  );
}
