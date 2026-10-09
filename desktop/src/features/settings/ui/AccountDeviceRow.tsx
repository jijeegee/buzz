import * as React from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Pencil } from "lucide-react";

import { useIdentityQuery } from "@/shared/api/hooks";
import { normalizeDeviceId } from "@/shared/api/ownerDevices";
import { type AuthDevice, renameAuthDevice } from "@/shared/api/tokenAuth";
import { applyAuthDeviceRename } from "@/shared/api/useAuthDeviceNames";
import {
  publishDeviceRobot,
  useOwnerDevices,
} from "@/shared/api/useOwnerDevices";
import { resolveDeviceRobotVariant } from "@/shared/lib/deviceRobot";
import { Button } from "@/shared/ui/button";
import { Input } from "@/shared/ui/input";

import { DeviceRobotPicker } from "./DeviceRobotPicker";

const DEVICE_NAME_MAX = 64;

/**
 * One signed-in device: its robot (the owner can pick another), a renamable
 * name, and (for other devices) a remote sign-out. `run` serializes actions
 * and reports their errors.
 */
export function AccountDeviceRow({
  device,
  disabled,
  run,
  onChanged,
  onSignOut,
  publishRobot = publishDeviceRobot,
}: {
  device: AuthDevice;
  disabled: boolean;
  run: (label: string, action: () => Promise<unknown>) => Promise<void>;
  onChanged: () => Promise<void>;
  onSignOut: () => void;
  /** Injection seam for tests; publishes the kind:30181 robot choice. */
  publishRobot?: typeof publishDeviceRobot;
}) {
  const queryClient = useQueryClient();
  const ownerPubkey = useIdentityQuery().data?.pubkey;
  const { robotOverrides } = useOwnerDevices();
  const variant = resolveDeviceRobotVariant(
    device.id,
    robotOverrides.get(normalizeDeviceId(device.id)),
  );
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
      applyAuthDeviceRename(queryClient, device.id, trimmed);
      await onChanged();
    });
  };

  return (
    <li className="flex items-center justify-between gap-3">
      <span className="flex min-w-0 flex-1 items-center gap-2 text-sm">
        <DeviceRobotPicker
          deviceId={device.id}
          deviceName={device.name}
          disabled={disabled || !ownerPubkey}
          onPick={(override) => {
            if (!ownerPubkey) return;
            void run("Change device robot", () =>
              publishRobot(queryClient, {
                ownerPubkey,
                deviceId: device.id,
                override,
              }),
            );
          }}
          variant={variant}
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
