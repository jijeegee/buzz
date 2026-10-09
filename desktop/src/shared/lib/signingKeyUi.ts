import type { TokenAuthStatus } from "@/shared/api/tokenAuth";

/**
 * Google sign-in is the only user-facing way into Buzz; the signing key behind
 * it is never shown. The key screens (create/import a key, key backup, raw
 * identity details) stay in the tree only for E2E mock builds
 * (`vite build --mode e2e`), whose mock bridge usually offers no Google
 * sign-in. Production and dev builds never render them.
 */
export function signingKeyUiTestSeam(
  mode: string | undefined = import.meta.env?.MODE,
): boolean {
  return mode === "e2e";
}

/**
 * Whether this device's Buzz account is held by Google: the signing key is
 * backed up with Google, or the community runs a Google token account (any
 * live, restoring or expired session, or a remembered legacy account).
 * Either way, signing in with Google again restores access.
 */
export function isGoogleBackedAccount(
  status: TokenAuthStatus | null | undefined,
): boolean {
  if (!status?.supported) return false;
  return (
    status.keyBackup === true ||
    status.legacyTokenAccount === true ||
    status.state !== "signed_out"
  );
}

/**
 * A local-only account on a relay that can hold it with Google: Settings
 * leads with an invitation to continue with Google instead of sign-out.
 */
export function needsGoogleProtection(
  status: TokenAuthStatus | null | undefined,
): boolean {
  return (
    status?.supported === true &&
    !isGoogleBackedAccount(status) &&
    status.keyBackupSupported === true
  );
}

/** Settings may show key identity/backup UI only through the E2E seam. */
export function showSigningKeySettings(
  status: TokenAuthStatus | null | undefined,
  mode?: string,
): boolean {
  return signingKeyUiTestSeam(mode) && !isGoogleBackedAccount(status);
}

export type OnboardingLandingMode =
  | "loading"
  | "google"
  | "signed-in"
  | "unavailable"
  | "key-fallback";

/**
 * What the first onboarding screen offers. Users only ever see Google sign-in
 * (or a retryable error when it cannot be reached); the key paths are an
 * E2E-only fallback for mock relays without Google sign-in.
 */
export function onboardingLandingMode(
  status: TokenAuthStatus | "loading" | "error",
  mode?: string,
): OnboardingLandingMode {
  if (status === "loading") return "loading";
  const supported = status !== "error" && status.supported;
  if (!supported) {
    return signingKeyUiTestSeam(mode) ? "key-fallback" : "unavailable";
  }
  return status.state === "active" ? "signed-in" : "google";
}
