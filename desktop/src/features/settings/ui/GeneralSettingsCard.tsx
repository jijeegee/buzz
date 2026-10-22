import { invoke } from "@tauri-apps/api/core";
import * as React from "react";
import { toast } from "sonner";

import { Switch } from "@/shared/ui/switch";
import { SettingsOptionGroup, SettingsOptionRow } from "./SettingsOptionGroup";

/**
 * Settings › General: app-wide behaviour of this computer's Kuzz. Open at
 * login is on by default; a start made at login stays in the tray.
 */
export function GeneralSettingsCard() {
  const [openAtLogin, setOpenAtLogin] = React.useState<boolean | null>(null);
  const [saving, setSaving] = React.useState(false);

  React.useEffect(() => {
    let cancelled = false;
    invoke<boolean>("get_open_at_login")
      .then((enabled) => {
        if (!cancelled) setOpenAtLogin(enabled);
      })
      .catch((error) =>
        console.error("[settings] open at login read failed:", error),
      );
    return () => {
      cancelled = true;
    };
  }, []);

  async function save(enabled: boolean) {
    setSaving(true);
    try {
      await invoke("set_open_at_login", { enabled });
      setOpenAtLogin(enabled);
    } catch (error) {
      console.error("[settings] open at login save failed:", error);
      toast.error("Could not change this setting. Try again.");
    } finally {
      setSaving(false);
    }
  }

  return (
    <div data-testid="settings-general">
      <SettingsOptionGroup title="Startup">
        <SettingsOptionRow>
          <div className="min-w-0">
            <label
              className="text-sm font-medium"
              htmlFor="settings-open-at-login"
            >
              Open Kuzz when your computer starts
            </label>
            <p
              className="text-sm font-normal text-muted-foreground/70"
              data-settings-subcopy
            >
              It starts in the background so your agents keep running.
            </p>
          </div>
          <Switch
            checked={openAtLogin ?? false}
            data-testid="settings-open-at-login"
            disabled={openAtLogin === null || saving}
            id="settings-open-at-login"
            onCheckedChange={(checked) => void save(checked)}
          />
        </SettingsOptionRow>
      </SettingsOptionGroup>
    </div>
  );
}
