import { getVersion } from "@tauri-apps/api/app";
import { openUrl } from "@tauri-apps/plugin-opener";
import * as React from "react";
import { useUpdaterContext } from "./hooks/UpdaterProvider";
import { useT } from "@/shared/i18n";
import { Button } from "@/shared/ui/button";
import {
  SettingsOptionGroup,
  SettingsOptionRow,
} from "./ui/SettingsOptionGroup";
import { SettingsSectionHeader } from "./ui/SettingsSectionHeader";
export function UpdateChecker() {
  const t = useT();
  const { status, checkForUpdate, installAndRelaunch } = useUpdaterContext();
  const [version, setVersion] = React.useState<string | null>(null);

  React.useEffect(() => {
    let cancelled = false;
    getVersion()
      .then((value) => {
        if (!cancelled) setVersion(value);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <section className="min-w-0" data-testid="settings-updates">
      <SettingsSectionHeader
        title={t("settings.updates.title")}
        description={t("settings.updates.description")}
      />

      <SettingsOptionGroup title={t("settings.updates.versionGroup")}>
        <SettingsOptionRow data-testid="settings-updates-version">
          <div className="min-w-0">
            <p className="text-sm font-medium">
              {t("settings.updates.currentVersion")}
            </p>
          </div>
          <p className="text-sm text-muted-foreground">
            {version ? `v${version}` : ""}
          </p>
        </SettingsOptionRow>
      </SettingsOptionGroup>

      <SettingsOptionGroup title={t("settings.updates.statusGroup")}>
        {status.state === "idle" && (
          <SettingsOptionRow>
            <div className="min-w-0">
              <p
                className="text-sm font-normal text-muted-foreground/70"
                data-settings-subcopy
              >
                {t("settings.updates.idleHint")}
              </p>
            </div>
            <Button size="sm" onClick={checkForUpdate}>
              {t("settings.updates.check")}
            </Button>
          </SettingsOptionRow>
        )}

        {status.state === "checking" && (
          <SettingsOptionRow>
            <div className="min-w-0">
              <p
                className="text-sm font-normal text-muted-foreground/70"
                data-settings-subcopy
              >
                {t("settings.updates.checking")}
              </p>
            </div>
          </SettingsOptionRow>
        )}

        {status.state === "up-to-date" && (
          <SettingsOptionRow>
            <div className="min-w-0">
              <p
                className="text-sm font-normal text-muted-foreground/70"
                data-settings-subcopy
              >
                {t("settings.updates.upToDate")}
              </p>
            </div>
            <Button variant="outline" size="sm" onClick={checkForUpdate}>
              {t("settings.updates.checkAgain")}
            </Button>
          </SettingsOptionRow>
        )}

        {status.state === "unavailable" && (
          <SettingsOptionRow>
            <div className="min-w-0">
              <p
                className="text-sm font-normal text-muted-foreground/70"
                data-settings-subcopy
              >
                {t("settings.updates.unavailable")}
              </p>
            </div>
            <Button variant="outline" size="sm" onClick={checkForUpdate}>
              {t("settings.updates.checkAgain")}
            </Button>
          </SettingsOptionRow>
        )}

        {status.state === "manual-required" && (
          <SettingsOptionRow>
            <div className="min-w-0">
              <p className="text-sm font-medium">
                {t("settings.updates.manualTitle", { version: status.version })}
              </p>
              <p
                className="text-sm font-normal text-muted-foreground/70"
                data-settings-subcopy
              >
                {t("settings.updates.manualHint")}{" "}
                <span>{t("settings.updates.manualAppImage")}</span>
              </p>
            </div>
            <Button size="sm" onClick={() => void openUrl(status.releaseUrl)}>
              {t("settings.updates.download")}
            </Button>
          </SettingsOptionRow>
        )}

        {status.state === "available" && (
          <SettingsOptionRow>
            <div className="min-w-0">
              <p
                className="text-sm font-normal text-muted-foreground/70"
                data-settings-subcopy
              >
                {t("settings.updates.preparing")}
              </p>
            </div>
          </SettingsOptionRow>
        )}

        {status.state === "downloading" && (
          <SettingsOptionRow>
            <div className="min-w-0">
              <p
                className="text-sm font-normal text-muted-foreground/70"
                data-settings-subcopy
              >
                {t("settings.updates.downloading")}
              </p>
            </div>
          </SettingsOptionRow>
        )}

        {status.state === "installing" && (
          <SettingsOptionRow>
            <div className="min-w-0">
              <p
                className="text-sm font-normal text-muted-foreground/70"
                data-settings-subcopy
              >
                {t("settings.updates.installing")}
              </p>
            </div>
          </SettingsOptionRow>
        )}

        {status.state === "ready" && (
          <SettingsOptionRow>
            <div className="min-w-0">
              <p
                className="text-sm font-normal text-muted-foreground/70"
                data-settings-subcopy
              >
                {t("settings.updates.ready")}
              </p>
            </div>
            <Button size="sm" onClick={installAndRelaunch}>
              {t("settings.updates.updateNow")}
            </Button>
          </SettingsOptionRow>
        )}

        {status.state === "error" && (
          <SettingsOptionRow>
            <div className="min-w-0">
              <p className="text-sm font-normal text-destructive">
                {t("settings.updates.failed", { message: status.message })}
              </p>
            </div>
            <Button variant="outline" size="sm" onClick={checkForUpdate}>
              {t("common.retry")}
            </Button>
          </SettingsOptionRow>
        )}
      </SettingsOptionGroup>
    </section>
  );
}
