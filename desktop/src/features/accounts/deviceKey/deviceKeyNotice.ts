import type { TokenAuthStatus } from "@/shared/api/tokenAuth";

/**
 * Why the active account should sign in once more to finish the device-key
 * security update: `missing` while its session still works but its device
 * has no key, `required` once the relay refused the keyless session.
 */
export type DeviceKeyNotice = "missing" | "required";

/** The relay's refusal of a keyless session, from a refresh or `/auth/me`. */
export function isDeviceKeyRequired(reason: string | null | undefined) {
  return (
    reason === "device_key_required" || reason === "me_device_key_required"
  );
}

/** Which notice the active account's status calls for, if any. Pure. */
export function deviceKeyNotice(
  status: Pick<TokenAuthStatus, "state" | "reason" | "deviceKeyMissing">,
): DeviceKeyNotice | null {
  if (status.state === "active" && status.deviceKeyMissing === true) {
    return "missing";
  }
  if (status.state === "needs_login" && isDeviceKeyRequired(status.reason)) {
    return "required";
  }
  return null;
}
