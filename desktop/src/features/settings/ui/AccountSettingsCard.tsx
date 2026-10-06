import * as React from "react";
import { toast } from "sonner";

import {
  type AuthDevice,
  deleteTokenAccount,
  getTokenAuthStatus,
  listAuthDevices,
  loginWithGoogle,
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
import { Button } from "@/shared/ui/button";
import { Spinner } from "@/shared/ui/spinner";
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
 * identity survives (AGENTS.md "Community Switching").
 */
function reloadIntoNewIdentity() {
  window.location.reload();
}

function shortId(hex: string | null): string {
  return hex ? `${hex.slice(0, 8)}…${hex.slice(-4)}` : "";
}

/**
 * Settings › Profile › Account: Google sign-in for the active community and,
 * once signed in, its devices and account-level security actions. Renders
 * nothing on a relay that does not offer token sign-in.
 */
export function AccountSettingsCard() {
  const [status, setStatus] = React.useState<TokenAuthStatus | null>(null);
  const [busy, setBusy] = React.useState<string | null>(null);

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
      const next = await loginWithGoogle(false, existingTokenAccount);
      setStatus(next);
      if (!next.keyBackup) reloadIntoNewIdentity();
    });

  const signOut = () =>
    run("Sign out", async () => {
      const next = await logoutTokenSession();
      setStatus(next);
      if (!next.keyBackup) reloadIntoNewIdentity();
    });

  return (
    <div className="mt-12" data-testid="settings-account">
      <SettingsOptionGroup
        description={
          (status.keyBackupSupported && !status.legacyTokenAccount) ||
          status.keyBackup
            ? "Google provides recovery for your existing Buzz signing key. The server operator or someone with access to your Google account can recover the key."
            : "This community uses Google sign-in. Your account, devices and agents are managed by the community's server."
        }
        title="Account"
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
                    : "Not signed in"}
              </p>
              <p className="text-sm text-muted-foreground/70">
                {status.state === "needs_login"
                  ? status.keyBackup
                    ? "Sign in again to manage recovery. Your signing key and agents keep working."
                    : "Sign in with Google again to keep using this community and its agents."
                  : status.keyBackupSupported && !status.legacyTokenAccount
                    ? "Link this existing Buzz identity to Google and back it up. A different backed-up identity cannot overwrite this one."
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
                ? "Waiting for browser…"
                : status.state === "needs_login"
                  ? "Sign in again"
                  : status.keyBackupSupported && !status.legacyTokenAccount
                    ? "Link key with Google"
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
                title="Return to an existing account created before Google key backup. This does not link or replace your Buzz signing key."
              >
                Use an existing token account
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
          <p className="font-medium">Signed in with Google</p>
          <p className="text-sm text-muted-foreground/70">
            {status.keyBackup ? "Backed-up signing key" : "Account"}{" "}
            {shortId(
              status.keyBackup
                ? (status.signingPubkey ?? null)
                : status.principal,
            )}
          </p>
        </div>
        <Button
          data-testid="account-sign-out"
          disabled={disabled}
          onClick={() =>
            void run("Sign out", async () => {
              const next = await logoutTokenSession();
              onStatus(next);
              if (!next.keyBackup) window.location.reload();
            })
          }
          type="button"
          variant="outline"
        >
          Sign out
        </Button>
      </SettingsOptionRow>
      {status.keyBackup ? (
        <SettingsOptionRow>
          <p className="min-w-0 text-sm text-muted-foreground/70">
            Signing out only blocks future backup access. It cannot revoke a
            copied signing key. Existing agents keep their keys and continue
            running.
          </p>
          <Button
            disabled={disabled}
            onClick={() =>
              void run("Verify backup", async () => {
                onStatus(await loginWithGoogle(false));
                toast.success(
                  "Google backup verified. Your signing identity is unchanged.",
                );
              })
            }
            type="button"
            variant="outline"
          >
            Verify Google backup
          </Button>
        </SettingsOptionRow>
      ) : null}
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
                <li
                  className="flex items-center justify-between gap-3"
                  key={device.id}
                >
                  <span className="min-w-0 truncate text-sm">
                    {device.name}
                    <span className="text-muted-foreground/70">
                      {" "}
                      · {device.platform}
                      {device.current ? " · this device" : ""}
                    </span>
                  </span>
                  {device.current ? null : (
                    <Button
                      aria-label={`Sign out ${device.name}`}
                      disabled={disabled}
                      onClick={() =>
                        void run("Sign out device", async () => {
                          await revokeAuthDevice(device.id);
                          await loadDevices();
                        })
                      }
                      size="sm"
                      type="button"
                      variant="outline"
                    >
                      Sign out
                    </Button>
                  )}
                </li>
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
          disabled={disabled}
          onClick={() =>
            void run("Sign out other devices", async () => {
              await revokeOtherSessions();
              await loadDevices();
              toast.success("Signed out of all other devices.");
            })
          }
          type="button"
          variant="outline"
        >
          Sign out other devices
        </Button>
      </SettingsOptionRow>
      {!status.keyBackup ? (
        <>
          <SettingsOptionRow>
            <p className="min-w-0 text-sm text-muted-foreground/70">
              Disconnect every agent right away. Agents on this computer get a
              new token the next time they start.
            </p>
            <Button
              disabled={disabled}
              onClick={() =>
                void run("Revoke agent tokens", async () => {
                  await revokeAllBotTokens();
                  toast.success("All agent tokens were revoked.");
                })
              }
              type="button"
              variant="outline"
            >
              Revoke agent tokens
            </Button>
          </SettingsOptionRow>
          <SettingsOptionRow>
            <p className="min-w-0 text-sm text-muted-foreground/70">
              Delete your account on this community. Signing in again within 30
              days restores it.
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
