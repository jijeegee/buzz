import * as React from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import {
  accountAfterSignOut,
  accountEmailDetail,
  accountLabel,
} from "@/features/accounts/lib/accountLabels";
import { signOutActiveAccount } from "@/features/accounts/lib/signOutActiveAccount";
import { useWatchedUnread } from "@/features/accounts/inactiveAccountWatch/useWatchedUnread";
import { AccountSwitcherSection } from "@/features/accounts/ui/AccountSwitcherSection";
import { useAccountMenu } from "@/features/accounts/useAccountMenu";
import { useIdentityQuery } from "@/shared/api/hooks";
import { type MessageKey, t as translate, useT } from "@/shared/i18n";
import { CreateAccountDialog } from "@/shared/ui/CreateAccountDialog";
import {
  type AuthDevice,
  deleteTokenAccount,
  getTokenAuthStatus,
  identityReload,
  listAccounts,
  listAuthDevices,
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
import { listOwnRunningLocalAgents } from "@/features/agents/lib/runningLocalAgents";
import { useGoogleLogin } from "@/shared/hooks/useGoogleLogin";
import { GoogleLoginProgress } from "@/shared/ui/GoogleLoginProgress";
import { Button } from "@/shared/ui/button";
import { Input } from "@/shared/ui/input";
import { needsGoogleProtection } from "@/shared/lib/signingKeyUi";
import { Spinner } from "@/shared/ui/spinner";
import {
  clearNewSignInDevice,
  newSignInDeviceId,
  newSignInDeviceName,
} from "@/features/accounts/newSignIn/newSignInHighlight";
import { EXTERNAL_PLATFORM } from "@/features/agents/externalConnect/pairingLogic";
import {
  refreshExternalAgents,
  useExternalAgentGroups,
} from "@/features/agents/externalConnect/useExternalAgents";
import { disconnectExternalDevice } from "@/shared/api/agentPairing";
import { AccountDeviceRow } from "./AccountDeviceRow";
import { SettingsConfirmDialog } from "./SettingsConfirmDialog";
import { SettingsOptionGroup, SettingsOptionRow } from "./SettingsOptionGroup";

/** The word typed in the second step of account deletion. */
export const ACCOUNT_DELETE_CONFIRM_WORD = "delete";

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
  identityReload.reload();
}

/** The sign-out text, naming the account that takes over if there is one. */
export function signOutDescription(nextAccount: string | null): string {
  return nextAccount
    ? translate("settings.account.signOutDescriptionSwitch", {
        account: nextAccount,
      })
    : translate("settings.account.signOutDescription");
}

/** "1 agent of this account will stop." / "N agents …". */
export function agentsWillStopLabel(count: number): string {
  return translate("settings.account.agentsWillStop", { count });
}

/**
 * Settings › Account: Google sign-in for the active community and, once
 * signed in, the accounts on this computer and account-level actions
 * (devices live in Settings › Devices). Renders
 * nothing on a relay that does not offer token sign-in. `onStatusChange`
 * lets the Profile panel adapt to whether Google holds the account.
 */
export function AccountSettingsCard({
  onStatusChange,
}: {
  onStatusChange?: (status: TokenAuthStatus | null) => void;
} = {}) {
  const t = useT();
  const login = useGoogleLogin();
  const [status, setStatus] = React.useState<TokenAuthStatus | null>(null);
  const [busy, setBusy] = React.useState<MessageKey | null>(null);

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
            translate("settings.account.agentsNewIdentity", {
              count: agents.length,
            }),
          );
        }
      })
      .catch(() => {});
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [refresh]);

  /** `label` marks the running action and is the message if it fails. */
  async function run(label: MessageKey, action: () => Promise<unknown>) {
    setBusy(label);
    try {
      await action();
    } catch (error) {
      toast.error(errorText(error, translate(label)));
    } finally {
      setBusy(null);
    }
  }

  if (!status?.supported) return null;

  const signIn = () =>
    run("settings.account.signInFailed", async () => {
      // A local-only account links its own key; otherwise the chosen Google
      // account decides which account opens.
      const connect = needsGoogleProtection(status);
      const next = await login.run(false, false, connect);
      if (!next) return;
      setStatus(next);
      if (
        !next.keyBackup ||
        next.accountChanged ||
        next.signingPubkey !== status.signingPubkey
      )
        reloadIntoNewIdentity();
    });

  const signOut = () =>
    run("settings.account.signOutFailed", () =>
      signOutActiveAccount(setStatus),
    );

  // Not yet held by Google: invite the user to protect the account with it.
  const protect = needsGoogleProtection(status);

  return (
    <div className={protect ? "mb-8" : "mt-12"} data-testid="settings-account">
      <SettingsOptionGroup
        description={
          protect
            ? t("settings.account.protectDescription")
            : t("settings.account.description")
        }
        title={
          protect
            ? t("settings.account.protectTitle")
            : t("settings.account.title")
        }
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
                  ? t("settings.account.restoring")
                  : status.state === "needs_login"
                    ? t("settings.account.needsLogin")
                    : protect
                      ? t("settings.account.notConnected")
                      : t("settings.account.notSignedIn")}
              </p>
              <p className="text-sm text-muted-foreground/70">
                {status.state === "needs_login"
                  ? status.keyBackup
                    ? t("settings.account.needsLoginHintKept")
                    : t("settings.account.needsLoginHint")
                  : protect
                    ? t("settings.account.protectHint")
                    : t("settings.account.signInHint")}
              </p>
            </div>
            <Button
              data-testid="account-sign-in"
              disabled={busy !== null || status.state === "restoring"}
              onClick={() => void signIn()}
              type="button"
            >
              {busy === "settings.account.signInFailed" ? (
                <Spinner aria-hidden className="h-4 w-4 border-2" />
              ) : null}
              {busy === "settings.account.signInFailed"
                ? t("settings.account.signingIn")
                : status.state === "needs_login"
                  ? t("settings.account.signInAgain")
                  : protect
                    ? t("settings.account.continueWithGoogle")
                    : t("settings.account.signInWithGoogle")}
            </Button>
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
                {t("settings.account.signOut")}
              </Button>
            ) : null}
          </SettingsOptionRow>
        )}
        <GoogleLoginProgress login={login} />
      </SettingsOptionGroup>
    </div>
  );
}

/**
 * The accounts signed in on this computer, with the profile menu's switch and
 * "Add account" (same list, same actions). Signing out stays on the row above.
 */
function AccountsOnThisComputer({ disabled }: { disabled: boolean }) {
  const t = useT();
  const accountMenu = useAccountMenu({ open: true });
  const activePubkey = useIdentityQuery().data?.pubkey ?? null;
  const unread = useWatchedUnread();
  return (
    <SettingsOptionRow className="items-start">
      <div className="min-w-0 flex-1" data-testid="settings-accounts">
        <p className="px-3 pb-1 font-medium">
          {t("settings.account.accountsOnThisComputer")}
        </p>
        <AccountSwitcherSection
          accounts={accountMenu.accounts}
          busy={accountMenu.busy || disabled}
          onSignIn={() => void accountMenu.signIn(activePubkey)}
          onSwitch={(account) => void accountMenu.switchTo(account)}
          unread={unread}
        />
      </div>
      <CreateAccountDialog login={accountMenu.signupPrompt} />
    </SettingsOptionRow>
  );
}

function ActiveAccount({
  status,
  busy,
  run,
  onStatus,
}: {
  status: TokenAuthStatus;
  busy: MessageKey | null;
  run: (label: MessageKey, action: () => Promise<unknown>) => Promise<void>;
  onStatus: (status: TokenAuthStatus) => void;
}) {
  const t = useT();
  // Deletion takes two steps: what happens, then typing the word.
  const [deleteStep, setDeleteStep] = React.useState<"explain" | "type" | null>(
    null,
  );
  const [deleteWord, setDeleteWord] = React.useState("");
  const closeDelete = (open: boolean) => {
    if (open) return;
    setDeleteStep(null);
    setDeleteWord("");
  };
  const deleteWordTyped =
    deleteWord.trim().toLowerCase() === ACCOUNT_DELETE_CONFIRM_WORD;
  const [confirming, setConfirming] = React.useState<"sign-out" | null>(null);
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

  const disabled = busy !== null;

  return (
    <>
      <SettingsOptionRow>
        <div className="min-w-0">
          <p className="truncate font-medium" data-testid="account-name">
            {name ?? t("settings.account.signedInWithGoogle")}
          </p>
          {name ? (
            <p className="truncate text-sm text-muted-foreground/70">
              {email ?? t("settings.account.signedInWithGoogle")}
            </p>
          ) : null}
        </div>
        <Button
          data-testid="account-sign-out"
          disabled={disabled}
          onClick={openSignOut}
          type="button"
          variant="outline"
        >
          {t("settings.account.signOut")}
        </Button>
      </SettingsOptionRow>
      <AccountsOnThisComputer disabled={disabled} />
      <SettingsConfirmDialog
        confirmLabel={t("settings.account.signOut")}
        description={signOutDescription(next ? accountLabel(next) : null)}
        onConfirm={() =>
          void run("settings.account.signOutFailed", () =>
            signOutActiveAccount(onStatus),
          )
        }
        onOpenChange={closeConfirm}
        open={confirming === "sign-out"}
        testId="account-sign-out-dialog"
        title={
          name
            ? t("settings.account.signOutTitleNamed", { name })
            : t("settings.account.signOutTitle")
        }
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
      {/* The relay does not yet delete Google key accounts. */}
      {!status.keyBackup ? (
        <>
          <details className="px-4 py-3">
            <summary className="cursor-pointer text-sm text-muted-foreground">
              {t("settings.account.deletionSummary")}
            </summary>
            <SettingsOptionRow>
              <p className="min-w-0 text-sm text-muted-foreground/70">
                {t("settings.account.deleteHint")}
              </p>
              <Button
                data-testid="account-delete-open"
                disabled={disabled}
                onClick={() => setDeleteStep("explain")}
                type="button"
                variant="destructive"
              >
                {t("settings.account.deleteAccount")}
              </Button>
            </SettingsOptionRow>
          </details>
          <AlertDialog onOpenChange={closeDelete} open={deleteStep !== null}>
            <AlertDialogContent data-testid="account-delete-dialog">
              <AlertDialogHeader>
                <AlertDialogTitle>
                  {t("settings.account.deleteTitle")}
                </AlertDialogTitle>
                <AlertDialogDescription>
                  {t("settings.account.deleteDescription")}
                </AlertDialogDescription>
              </AlertDialogHeader>
              {deleteStep === "type" ? (
                <div className="space-y-2">
                  <label
                    className="text-sm font-medium"
                    htmlFor="account-delete-word"
                  >
                    {t("settings.account.deleteTypeLabel", {
                      word: ACCOUNT_DELETE_CONFIRM_WORD,
                    })}
                  </label>
                  <Input
                    autoComplete="off"
                    autoFocus
                    data-testid="account-delete-word"
                    disabled={disabled}
                    id="account-delete-word"
                    onChange={(event) => setDeleteWord(event.target.value)}
                    placeholder={ACCOUNT_DELETE_CONFIRM_WORD}
                    spellCheck={false}
                    value={deleteWord}
                  />
                </div>
              ) : null}
              <AlertDialogFooter>
                <AlertDialogCancel disabled={disabled}>
                  {t("common.cancel")}
                </AlertDialogCancel>
                {deleteStep === "explain" ? (
                  <Button
                    data-testid="account-delete-continue"
                    onClick={() => setDeleteStep("type")}
                    type="button"
                    variant="destructive"
                  >
                    {t("settings.account.deleteContinue")}
                  </Button>
                ) : (
                  <Button
                    data-testid="account-delete-confirm"
                    disabled={disabled || !deleteWordTyped}
                    onClick={() =>
                      void run("settings.account.deleteFailed", async () => {
                        await deleteTokenAccount();
                        identityReload.reload();
                      })
                    }
                    type="button"
                    variant="destructive"
                  >
                    {t("settings.account.deleteAccount")}
                  </Button>
                )}
              </AlertDialogFooter>
            </AlertDialogContent>
          </AlertDialog>
        </>
      ) : null}
    </>
  );
}

/**
 * Settings › Devices: the computers and phones signed in to the active
 * account — rename, pick each one's robot, sign one out, or sign out every
 * other device. Renders nothing on a relay without token sign-in.
 */
export function DevicesSettingsCard() {
  const t = useT();
  const [status, setStatus] = React.useState<TokenAuthStatus | null>(null);
  const [busy, setBusy] = React.useState<MessageKey | null>(null);

  React.useEffect(() => {
    const refresh = () =>
      void getTokenAuthStatus()
        .then(setStatus)
        .catch((error) =>
          console.error("[devices] token auth status failed:", error),
        );
    refresh();
    let unlisten: (() => void) | undefined;
    let disposed = false;
    void onTokenAuthChanged(refresh).then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  /** `label` marks the running action and is the message if it fails. */
  async function run(label: MessageKey, action: () => Promise<unknown>) {
    setBusy(label);
    try {
      await action();
    } catch (error) {
      toast.error(errorText(error, translate(label)));
    } finally {
      setBusy(null);
    }
  }

  if (!status?.supported) return null;

  return (
    <div data-testid="settings-devices">
      <SettingsOptionGroup
        description={t("settings.device.description")}
        title={t("settings.device.cardTitle")}
      >
        {status.state === "active" ? (
          <SignedInDevices agents={!status.keyBackup} busy={busy} run={run} />
        ) : (
          <SettingsOptionRow>
            <p className="min-w-0 text-sm text-muted-foreground/70">
              {t("settings.device.signInHint")}
            </p>
          </SettingsOptionRow>
        )}
      </SettingsOptionGroup>
    </div>
  );
}

function SignedInDevices({
  agents,
  busy,
  run,
}: {
  /** Offer disconnecting every agent. */
  agents: boolean;
  busy: MessageKey | null;
  run: (label: MessageKey, action: () => Promise<unknown>) => Promise<void>;
}) {
  const t = useT();
  const [devices, setDevices] = React.useState<AuthDevice[] | null>(null);
  const [confirming, setConfirming] = React.useState<
    "sign-out-others" | "disconnect-agents" | null
  >(null);
  const closeConfirm = (open: boolean) => {
    if (!open) setConfirming(null);
  };
  const loadDevices = React.useCallback(async () => {
    try {
      setDevices(await listAuthDevices());
    } catch (error) {
      toast.error(errorText(error, translate("settings.device.loadFailed")));
    }
  }, []);

  React.useEffect(() => {
    void loadDevices();
  }, [loadDevices]);

  const disabled = busy !== null;
  const queryClient = useQueryClient();
  const hermesDevices = useExternalAgentGroups().hermesDeviceIds;
  // Reached from a "New sign-in" notice: mark that device, found by name
  // once when the list loads and then kept by id. Seeing this view uses the
  // notice up; leaving it clears the mark.
  const [newSignInName, setNewSignInName] = React.useState(newSignInDeviceName);
  const [newDeviceId, setNewDeviceId] = React.useState<string | null>(null);
  React.useEffect(() => {
    clearNewSignInDevice();
  }, []);
  React.useEffect(() => {
    if (devices === null || newSignInName === null) return;
    setNewDeviceId(newSignInDeviceId(devices, newSignInName));
    setNewSignInName(null);
  }, [devices, newSignInName]);

  return (
    <>
      <SettingsOptionRow className="items-start">
        <div className="min-w-0 flex-1">
          <p className="font-medium" id="account-devices-heading">
            {t("settings.device.title")}
          </p>
          {devices === null ? (
            <p className="text-sm text-muted-foreground/70">
              {t("common.loading")}
            </p>
          ) : (
            <ul
              aria-labelledby="account-devices-heading"
              className="mt-2 space-y-2"
            >
              {devices.map((device) => (
                <AccountDeviceRow
                  device={device}
                  disabled={disabled}
                  hermes={
                    device.platform === EXTERNAL_PLATFORM &&
                    hermesDevices.has(device.id)
                  }
                  isNew={device.id === newDeviceId}
                  key={device.id}
                  onChanged={loadDevices}
                  onSignOut={() =>
                    device.platform === EXTERNAL_PLATFORM
                      ? void run(
                          "settings.device.disconnectFailed",
                          async () => {
                            try {
                              await disconnectExternalDevice(device.id);
                            } finally {
                              refreshExternalAgents(queryClient);
                            }
                            await loadDevices();
                          },
                        )
                      : void run(
                          "settings.device.signOutDeviceFailed",
                          async () => {
                            await revokeAuthDevice(device.id);
                            await loadDevices();
                          },
                        )
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
          {t("settings.device.signOutOthersHint")}
        </p>
        <Button
          data-testid="account-sign-out-others"
          disabled={disabled}
          onClick={() => setConfirming("sign-out-others")}
          type="button"
          variant="outline"
        >
          {t("settings.device.signOutOthers")}
        </Button>
      </SettingsOptionRow>
      <SettingsConfirmDialog
        confirmLabel={t("settings.device.signOutOthers")}
        description={t("settings.device.signOutOthersDescription")}
        onConfirm={() =>
          void run("settings.device.signOutOthersFailed", async () => {
            await revokeOtherSessions();
            await loadDevices();
            toast.success(t("settings.device.signedOutOthers"));
          })
        }
        onOpenChange={closeConfirm}
        open={confirming === "sign-out-others"}
        testId="account-sign-out-others-dialog"
        title={t("settings.device.signOutOthersTitle")}
      />
      {/* The relay cannot yet cut off a Google key account's agents. */}
      {agents ? (
        <>
          <SettingsOptionRow>
            <p className="min-w-0 text-sm text-muted-foreground/70">
              {t("settings.account.disconnectHint")}
            </p>
            <Button
              data-testid="account-disconnect-agents"
              disabled={disabled}
              onClick={() => setConfirming("disconnect-agents")}
              type="button"
              variant="outline"
            >
              {t("settings.account.disconnectAll")}
            </Button>
          </SettingsOptionRow>
          <SettingsConfirmDialog
            confirmLabel={t("settings.account.disconnectConfirm")}
            description={t("settings.account.disconnectDescription")}
            destructive
            onConfirm={() =>
              void run("settings.account.disconnectFailed", async () => {
                await revokeAllBotTokens();
                toast.success(t("settings.account.disconnected"));
              })
            }
            onOpenChange={closeConfirm}
            open={confirming === "disconnect-agents"}
            testId="account-disconnect-agents-dialog"
            title={t("settings.account.disconnectTitle")}
          />
        </>
      ) : null}
    </>
  );
}
