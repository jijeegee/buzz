import type { ReactNode } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";

import { useUpdaterContext } from "./hooks/UpdaterProvider";
import { useT } from "@/shared/i18n";
import { Button } from "@/shared/ui/button";
import { Spinner } from "@/shared/ui/spinner";

/**
 * Covers the whole app while this build is below the minimum supported
 * version. The only way forward is to update; the download already runs in
 * the background, so the button turns into "Update and restart" when ready.
 */
export function RequiredUpdateScreen() {
  const t = useT();
  const { required, status, checkForUpdate, installAndRelaunch } =
    useUpdaterContext();

  if (!required) {
    return null;
  }

  const busy =
    status.state === "checking" ||
    status.state === "available" ||
    status.state === "downloading" ||
    status.state === "installing";

  let action: ReactNode;
  if (status.state === "ready") {
    action = (
      <Button onClick={() => void installAndRelaunch()}>
        {t("settings.requiredUpdate.install")}
      </Button>
    );
  } else if (status.state === "manual-required" && status.releaseUrl) {
    const { releaseUrl } = status;
    action = (
      <Button onClick={() => void openUrl(releaseUrl)}>
        {t("settings.requiredUpdate.download")}
      </Button>
    );
  } else if (busy) {
    action = (
      <Button disabled>
        <Spinner aria-hidden="true" className="h-4 w-4 border-2" />
        {status.state === "installing"
          ? t("settings.requiredUpdate.installing")
          : t("settings.requiredUpdate.downloading")}
      </Button>
    );
  } else {
    action = (
      <Button onClick={() => void checkForUpdate()}>
        {t("settings.requiredUpdate.retry")}
      </Button>
    );
  }

  return (
    <div
      aria-modal="true"
      className="fixed inset-0 z-[1000] flex items-center justify-center bg-background"
      data-testid="required-update-screen"
      role="dialog"
    >
      <div className="flex max-w-sm flex-col items-center gap-4 px-6 text-center">
        <h1 className="text-xl font-semibold">
          {t("settings.requiredUpdate.title")}
        </h1>
        <p className="text-sm text-muted-foreground">
          {t("settings.requiredUpdate.description")}
        </p>
        {action}
        {status.state === "error" ? (
          <p className="text-xs text-muted-foreground/70">{status.message}</p>
        ) : null}
      </div>
    </div>
  );
}
