import * as React from "react";
import { toast } from "sonner";

import { beginAccountAction } from "@/features/accounts/lib/accountSwitch";
import { accountLabel } from "@/features/accounts/lib/accountLabels";
import {
  type AccountSummary,
  listAccounts,
  switchAccount,
} from "@/shared/api/tokenAuth";
import { useT } from "@/shared/i18n";
import { useSystemColorScheme } from "@/shared/theme/useSystemColorScheme";
import { Button } from "@/shared/ui/button";
import { StartupWindowDragRegion } from "@/shared/ui/StartupWindowDragRegion";
import { GoogleSignInButton } from "./GoogleSignInButton";

/**
 * Shown instead of the app while this computer is signed out of the active
 * community. Nothing was deleted; signing in with Google gets back in. The
 * signed-out account's agents stopped with it, so none are reported here.
 * Accounts still signed in on this computer are offered to switch to.
 */
export function SignedOutScreen() {
  const t = useT();
  const systemColorScheme = useSystemColorScheme();
  const [others, setOthers] = React.useState<AccountSummary[]>([]);
  const [switching, setSwitching] = React.useState(false);

  React.useEffect(() => {
    let cancelled = false;
    listAccounts()
      .then((accounts) => {
        if (cancelled) return;
        setOthers(
          accounts.filter(
            (account) => !account.active && account.state === "signed_in",
          ),
        );
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, []);

  async function switchTo(account: AccountSummary) {
    setSwitching(true);
    const action = beginAccountAction();
    try {
      await switchAccount(account.pubkey);
      action.reload(
        t("accounts.switch.switchingTo", { name: accountLabel(account) }),
      );
    } catch (error) {
      action.cancel();
      setSwitching(false);
      toast.error(
        error instanceof Error
          ? error.message
          : t("accounts.menu.switchFailed"),
      );
    }
  }

  return (
    <div
      className="kuzz-onboarding-neutral-theme kuzz-startup-shell flex items-center justify-center bg-background px-4 py-8 text-foreground"
      data-system-color-scheme={systemColorScheme}
      data-testid="signed-out-screen"
    >
      <StartupWindowDragRegion />
      <main
        aria-labelledby="signed-out-title"
        className="relative flex w-full max-w-sm flex-col items-center text-center"
      >
        <h1
          className="text-3xl font-semibold tracking-tight"
          id="signed-out-title"
        >
          {t("onboarding.signedOut.title")}
        </h1>
        <p className="mt-3 text-sm leading-6 text-muted-foreground">
          {t("onboarding.signedOut.body")}
        </p>
        <div className="mt-8 w-full">
          <GoogleSignInButton
            allowRestore
            note={t("onboarding.signedOut.note")}
            showWhenSignedIn
            unavailable={
              <div className="space-y-3">
                <p className="text-sm text-muted-foreground" role="status">
                  {t("onboarding.signedOut.unavailable")}
                </p>
                <Button
                  className="h-10 w-full"
                  onClick={() => window.location.reload()}
                  type="button"
                  variant="outline"
                >
                  {t("onboarding.signedOut.tryAgain")}
                </Button>
              </div>
            }
          />
        </div>
        {others.length > 0 ? (
          <section
            aria-labelledby="signed-out-other-accounts"
            className="mt-8 w-full space-y-2"
            data-testid="signed-out-other-accounts"
          >
            <h2
              className="text-sm text-muted-foreground"
              id="signed-out-other-accounts"
            >
              {t("onboarding.signedOut.otherAccounts")}
            </h2>
            {others.map((account) => {
              const label = accountLabel(account);
              return (
                <Button
                  className="h-10 w-full"
                  data-testid={`signed-out-switch-${account.pubkey}`}
                  disabled={switching}
                  key={account.pubkey}
                  onClick={() => void switchTo(account)}
                  type="button"
                  variant="outline"
                >
                  <span className="min-w-0 truncate">
                    {t("onboarding.signedOut.continueAs", { name: label })}
                  </span>
                </Button>
              );
            })}
          </section>
        ) : null}
      </main>
    </div>
  );
}

/**
 * The community app's mount boundary: while this computer is signed out, the
 * chat shell (and every relay connection it would open) never mounts.
 */
export function SignedOutGate({
  signedOut,
  children,
}: {
  signedOut: boolean;
  children: React.ReactNode;
}) {
  return signedOut ? <SignedOutScreen /> : children;
}
