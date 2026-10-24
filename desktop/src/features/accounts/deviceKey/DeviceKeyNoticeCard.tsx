import * as React from "react";
import { ShieldCheck } from "lucide-react";
import { toast } from "sonner";

import { accountProvider } from "@/features/accounts/lib/accountLabels";
import { beginAccountAction } from "@/features/accounts/lib/accountSwitch";
import {
  getTokenAuthStatus,
  listAccounts,
  logoutTokenSession,
  onTokenAuthChanged,
  type TokenAuthStatus,
} from "@/shared/api/tokenAuth";
import { useGoogleLogin } from "@/shared/hooks/useGoogleLogin";
import { useT } from "@/shared/i18n";
import { GoogleLoginProgress } from "@/shared/ui/GoogleLoginProgress";
import { SidebarCompactActionCard } from "@/shared/ui/sidebar-action-card";
import { type DeviceKeyNotice, deviceKeyNotice } from "./deviceKeyNotice";

/**
 * Sidebar card asking the active account to sign in once more when its
 * device has no key yet (signed in before device keys), or when the relay
 * already refused that keyless session. Only this account signs out and back
 * in; other accounts and their agents are untouched.
 */
export function DeviceKeyNoticeCard() {
  const t = useT();
  const login = useGoogleLogin();
  const [status, setStatus] = React.useState<TokenAuthStatus | null>(null);
  const [dismissed, setDismissed] = React.useState(false);
  // The notice being acted on: kept while signing out and back in, when the
  // status no longer calls for it (and the sign-in must stay mounted).
  const [acting, setActing] = React.useState<DeviceKeyNotice | null>(null);

  React.useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    const refresh = () =>
      void getTokenAuthStatus()
        .then((next) => {
          if (!disposed) setStatus(next);
        })
        .catch(() => {});
    refresh();
    void onTokenAuthChanged(refresh).then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  const notice = acting ?? (status ? deviceKeyNotice(status) : null);
  if (!status || !notice || (dismissed && !acting)) return null;

  async function signInAgain(current: TokenAuthStatus, which: DeviceKeyNotice) {
    setActing(which);
    const action = beginAccountAction();
    let signedOut = false;
    try {
      const accounts = await listAccounts().catch(() => []);
      const active = accounts.find((account) => account.active);
      const provider = active ? accountProvider(active) : "google";
      // A session that still works is ended first, so the new sign-in
      // registers this device's key; a refused one already ended.
      if (which === "missing") {
        await logoutTokenSession(current.signingPubkey ?? undefined);
        signedOut = true;
      }
      const next = await login.run(false, false, false, provider);
      if (signedOut || next) {
        action.reload(null);
        return;
      }
      action.cancel();
    } catch (error) {
      if (signedOut) {
        action.reload(null);
        return;
      }
      action.cancel();
      toast.error(
        error instanceof Error
          ? error.message
          : t("accounts.menu.googleSignInFailed"),
      );
    }
    setActing(null);
  }

  return (
    <div className="mb-2 space-y-2 group-data-[collapsible=icon]:hidden">
      <SidebarCompactActionCard
        actionAriaLabel={t("accounts.deviceKey.action")}
        actionDisabled={acting !== null}
        actionTestId="device-key-notice-action"
        description={t("accounts.deviceKey.action")}
        descriptionClassName="underline underline-offset-2"
        dismissLabel={t("accounts.deviceKey.dismiss")}
        icon={<ShieldCheck aria-hidden="true" className="h-5 w-5" />}
        iconKey={`device-key-${notice}`}
        onAction={() => void signInAgain(status, notice)}
        onDismiss={() => setDismissed(true)}
        role="status"
        testId="device-key-notice"
        title={
          notice === "missing"
            ? t("accounts.deviceKey.title")
            : t("accounts.deviceKey.requiredTitle")
        }
      />
      <GoogleLoginProgress login={login} />
    </div>
  );
}
