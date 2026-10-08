import * as React from "react";
import { Pencil } from "lucide-react";

import { type AuthDevice, renameAuthDevice } from "@/shared/api/tokenAuth";
import { notifyAuthDevicesChanged } from "@/shared/api/useAuthDeviceNames";
import { deviceRobotVariantForDevice } from "@/shared/lib/deviceRobot";
import { Button } from "@/shared/ui/button";
import { DeviceRobotIcon } from "@/shared/ui/DeviceRobotIcon";
import { Input } from "@/shared/ui/input";

const DEVICE_NAME_MAX = 64;

/**
 * One signed-in device: its robot, a renamable name, and (for other devices)
 * a remote sign-out. `run` serializes actions and reports their errors.
 */
export function AccountDeviceRow({
  device,
  disabled,
  run,
  onChanged,
  onSignOut,
}: {
  device: AuthDevice;
  disabled: boolean;
  run: (label: string, action: () => Promise<unknown>) => Promise<void>;
  onChanged: () => Promise<void>;
  onSignOut: () => void;
}) {
  const [draft, setDraft] = React.useState<string | null>(null);
  const trimmed = draft?.trim() ?? "";

  const save = () => {
    if (draft === null) return;
    if (trimmed.length === 0 || trimmed === device.name) {
      setDraft(null);
      return;
    }
    void run("Rename device", async () => {
      await renameAuthDevice(device.id, trimmed);
      setDraft(null);
      await onChanged();
      notifyAuthDevicesChanged();
    });
  };

  return (
    <li className="flex items-center justify-between gap-3">
      <span className="flex min-w-0 flex-1 items-center gap-2 text-sm">
        <DeviceRobotIcon
          className="h-5 w-5"
          variant={deviceRobotVariantForDevice(device.id)}
        />
        {draft === null ? (
          <>
            <span className="min-w-0 truncate">
              {device.name}
              <span className="text-muted-foreground/70">
                {" "}
                · {device.platform}
                {device.current ? " · this device" : ""}
              </span>
            </span>
            <Button
              aria-label={`Rename ${device.name}`}
              className="h-6 w-6 shrink-0"
              disabled={disabled}
              onClick={() => setDraft(device.name)}
              size="icon"
              type="button"
              variant="ghost"
            >
              <Pencil aria-hidden="true" className="h-3.5 w-3.5" />
            </Button>
          </>
        ) : (
          <form
            className="flex min-w-0 flex-1 items-center gap-2"
            onSubmit={(event) => {
              event.preventDefault();
              save();
            }}
          >
            <Input
              aria-label={`New name for ${device.name}`}
              autoFocus
              className="h-7 min-w-0 flex-1 text-sm"
              disabled={disabled}
              maxLength={DEVICE_NAME_MAX}
              onChange={(event) => setDraft(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Escape") {
                  event.preventDefault();
                  setDraft(null);
                }
              }}
              value={draft}
            />
            <Button
              disabled={disabled || trimmed.length === 0}
              size="sm"
              type="submit"
            >
              Save
            </Button>
            <Button
              disabled={disabled}
              onClick={() => setDraft(null)}
              size="sm"
              type="button"
              variant="ghost"
            >
              Cancel
            </Button>
          </form>
        )}
      </span>
      {device.current || draft !== null ? null : (
        <Button
          aria-label={`Sign out ${device.name}`}
          disabled={disabled}
          onClick={onSignOut}
          size="sm"
          type="button"
          variant="outline"
        >
          Sign out
        </Button>
      )}
    </li>
  );
}
