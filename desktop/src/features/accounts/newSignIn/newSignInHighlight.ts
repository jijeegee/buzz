import type { AuthDevice } from "@/shared/api/tokenAuth";

/** The device name of the "New sign-in" notice the owner followed. */
let followedName: string | null = null;

/** Remember the notice's device, so the device list can mark it "New". */
export function markNewSignInDevice(name: string) {
  followedName = name.trim() || null;
}

export function newSignInDeviceName(): string | null {
  return followedName;
}

/**
 * The device the notice was about: the newest (last listed) other device
 * with that name, or `null`. Pure.
 */
export function newSignInDeviceId(
  devices: readonly AuthDevice[],
  name: string | null,
): string | null {
  if (!name) return null;
  for (let index = devices.length - 1; index >= 0; index -= 1) {
    const device = devices[index];
    if (!device.current && device.name.trim() === name) return device.id;
  }
  return null;
}
