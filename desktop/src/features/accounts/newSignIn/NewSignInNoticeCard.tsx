import { MonitorSmartphone } from "lucide-react";

import { useAppNavigation } from "@/app/navigation/useAppNavigation";
import { useT } from "@/shared/i18n";
import { SidebarCompactActionCard } from "@/shared/ui/sidebar-action-card";
import { markNewSignInDevice } from "./newSignInHighlight";
import { useNewSignInNotice } from "./useNewSignInNotice";

/** Sidebar card shown when the account was just signed in on another device. */
export function NewSignInNoticeCard({
  pubkey,
}: {
  pubkey: string | undefined;
}) {
  const { notice, dismiss } = useNewSignInNotice(pubkey);
  const { goSettings } = useAppNavigation();
  const t = useT();

  if (!notice) return null;

  return (
    <div className="mb-2 group-data-[collapsible=icon]:hidden">
      <SidebarCompactActionCard
        actionAriaLabel={t("accounts.newSignIn.action")}
        actionTestId="new-signin-notice-action"
        description={t("accounts.newSignIn.action")}
        descriptionClassName="underline underline-offset-2"
        dismissLabel={t("accounts.newSignIn.dismiss")}
        icon={<MonitorSmartphone aria-hidden="true" className="h-5 w-5" />}
        iconKey="new-signin"
        onAction={() => {
          markNewSignInDevice(notice.deviceName);
          dismiss();
          void goSettings("devices");
        }}
        onDismiss={dismiss}
        testId="new-signin-notice"
        title={t("accounts.newSignIn.title", { device: notice.deviceName })}
      />
    </div>
  );
}
