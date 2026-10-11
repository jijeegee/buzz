import * as React from "react";
import { useQuery } from "@tanstack/react-query";
import { toast } from "sonner";

import {
  type AccountSummary,
  listAccounts,
  switchAccount,
} from "@/shared/api/tokenAuth";
import { useGoogleLogin } from "@/shared/hooks/useGoogleLogin";
import {
  beginAccountAction,
  finishAccountSwitchBoot,
} from "./lib/accountSwitch";
import { accountLabel, menuAccounts } from "./lib/accountLabels";

function errorText(error: unknown, fallback: string): string {
  return error instanceof Error
    ? error.message
    : typeof error === "string" && error
      ? error
      : fallback;
}

export const SWITCH_INCOMPLETE_NOTICE =
  "Switched accounts. Some communities could not move to this account yet; sign in again there if they ask.";

/**
 * The profile menu's accounts: list, switch, and add (or sign in again).
 * Lives with the sidebar card, not the menu, so a Google sign-in keeps going
 * after the menu closes.
 */
export function useAccountMenu({ open }: { open: boolean }) {
  const query = useQuery({
    queryKey: ["accounts"],
    queryFn: listAccounts,
    staleTime: 0,
  });
  const { refetch } = query;
  React.useEffect(() => {
    if (open) void refetch();
  }, [open, refetch]);

  // A notice left by the switch that restarted the app.
  React.useEffect(() => {
    const notice = finishAccountSwitchBoot();
    if (notice) toast.warning(notice);
  }, []);

  const login = useGoogleLogin();
  const loginRef = React.useRef(login);
  loginRef.current = login;
  const [busy, setBusy] = React.useState(false);
  const progressToast = React.useRef<string | number | null>(null);

  // The new-account screen replaces the "finish in your browser" notice.
  React.useEffect(() => {
    if (login.signup && progressToast.current !== null) {
      toast.dismiss(progressToast.current);
      progressToast.current = null;
    }
  }, [login.signup]);

  React.useEffect(() => {
    if (login.error) toast.error(login.error);
  }, [login.error]);

  const switchTo = React.useCallback(async (account: AccountSummary) => {
    setBusy(true);
    const action = beginAccountAction();
    try {
      const incomplete = await switchAccount(account.pubkey);
      action.reload(
        `Switching to ${accountLabel(account)}…`,
        incomplete ? SWITCH_INCOMPLETE_NOTICE : null,
      );
    } catch (error) {
      action.cancel();
      setBusy(false);
      toast.error(errorText(error, "Could not switch accounts."));
    }
  }, []);

  /**
   * Google sign-in for another account (or a fresh sign-in for one whose
   * sign-in ended). The account it opens becomes the active one.
   */
  const signIn = React.useCallback(
    async (activePubkey: string | null) => {
      setBusy(true);
      const action = beginAccountAction();
      const progress = toast.loading("Finish signing in in your browser…", {
        duration: Number.POSITIVE_INFINITY,
        action: {
          label: "Cancel",
          onClick: () => void loginRef.current.cancel(),
        },
      });
      progressToast.current = progress;
      try {
        const next = await loginRef.current.run(false);
        toast.dismiss(progress);
        const opened = next?.keyBackup ? next.signingPubkey : next?.principal;
        if (next && (next.accountChanged || opened !== activePubkey)) {
          action.reload("Switching accounts…");
          return;
        }
        action.cancel();
        setBusy(false);
        if (next) {
          toast.success("You're signed in to this account.");
          void refetch();
        }
      } catch (error) {
        toast.dismiss(progress);
        action.cancel();
        setBusy(false);
        toast.error(errorText(error, "Google sign-in failed. Try again."));
      }
    },
    [refetch],
  );

  return {
    accounts: menuAccounts(query.data ?? []),
    busy: busy || login.pending,
    switchTo,
    signIn,
    /** For the "Create your Buzz account" screen. */
    signupPrompt: { signup: login.signup, answerSignup: login.answerSignup },
  };
}
