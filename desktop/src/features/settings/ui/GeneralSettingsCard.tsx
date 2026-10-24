import * as React from "react";

import { invokeTauri } from "@/shared/api/tauri";
import { useT } from "@/shared/i18n";
import { isMacPlatform } from "@/shared/lib/platform";
import { Switch } from "@/shared/ui/switch";
import { LanguageSetting } from "./AppearanceSettingsControls";
import { SettingsSectionHeader } from "./SettingsSectionHeader";
import {
  SettingsOptionGroup,
  SettingsOptionGroupList,
  SettingsOptionRow,
} from "./SettingsOptionGroup";

/** Settings › General: the first page of Settings. */
export function GeneralSettingsCard() {
  const t = useT();
  return (
    <section
      className="flex min-h-0 flex-1 flex-col overflow-y-auto"
      data-testid="settings-general"
    >
      <SettingsSectionHeader
        title={t("settings.general.title")}
        description={t("settings.general.description")}
      />
      <SettingsOptionGroupList>
        <SettingsOptionGroup
          data-testid="general-language-card"
          title={t("settings.language.title")}
        >
          <LanguageSetting />
        </SettingsOptionGroup>
        <SettingsOptionGroup
          data-testid="general-startup-card"
          title={t("settings.general.startupTitle")}
        >
          <LaunchAtLoginSetting />
        </SettingsOptionGroup>
      </SettingsOptionGroupList>
    </section>
  );
}

/**
 * Open Kuzz when the user signs in to the computer. It starts in the tray
 * (menu bar on macOS) so agents run without the window popping up.
 */
function LaunchAtLoginSetting() {
  const t = useT();
  // null while the current OS setting loads.
  const [enabled, setEnabled] = React.useState<boolean | null>(null);
  const [saving, setSaving] = React.useState(false);
  const [error, setError] = React.useState<string | null>(null);

  React.useEffect(() => {
    let cancelled = false;
    invokeTauri<boolean>("get_launch_at_login")
      .then((value) => {
        if (!cancelled) setEnabled(value);
      })
      .catch((failure: unknown) => {
        if (!cancelled) {
          setEnabled(false);
          setError(String(failure));
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const change = (next: boolean) => {
    setSaving(true);
    setError(null);
    invokeTauri<boolean>("set_launch_at_login", { enabled: next })
      .then(setEnabled)
      .catch((failure: unknown) => setError(String(failure)))
      .finally(() => setSaving(false));
  };

  return (
    <SettingsOptionRow data-testid="launch-at-login-row">
      <div className="min-w-0">
        <label className="text-sm font-medium" htmlFor="launch-at-login-switch">
          {t("settings.general.launchAtLogin")}
        </label>
        <p
          className="text-sm font-normal text-muted-foreground/70"
          data-settings-subcopy
        >
          {isMacPlatform()
            ? t("settings.general.launchAtLoginHintMac")
            : t("settings.general.launchAtLoginHint")}
        </p>
        {error ? (
          <p className="text-sm text-destructive" role="alert">
            {t("settings.general.launchAtLoginFailed", { error })}
          </p>
        ) : null}
      </div>
      <Switch
        checked={enabled === true}
        data-testid="launch-at-login-toggle"
        disabled={enabled === null || saving}
        id="launch-at-login-switch"
        onCheckedChange={change}
      />
    </SettingsOptionRow>
  );
}
