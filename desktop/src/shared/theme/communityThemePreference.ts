import { normalizeRelayUrl } from "@/features/profile/lib/selfProfileStorage";
import { ACCENT_COLORS } from "./ThemeProvider";
import {
  SYNTAX_THEMES,
  getThemePair,
  isLightTheme,
  type SyntaxThemeName,
} from "./theme-loader";

const STORAGE_KEY_PREFIX = "buzz-community-theme.v1";
const OUTBOX_KEY_PREFIX = "buzz-community-theme-outbox.v1";
const MIGRATION_KEY_PREFIX = "buzz-community-theme-migrated.v1";
// Light/dark/system is a property of the device, not the account: it is kept
// here only and overlaid on every synced preference before it is applied.
const DEVICE_APPEARANCE_MODE_KEY = "buzz-device-appearance-mode.v1";

export type DeviceAppearanceMode = "system" | "light" | "dark";

export type CommunityThemePreference = {
  version: 1;
  theme: SyntaxThemeName;
  accent: string;
  followSystem: boolean;
};

export const DEFAULT_COMMUNITY_THEME: CommunityThemePreference = Object.freeze({
  version: 1,
  theme: "buzz",
  accent: "#3b82f6",
  followSystem: true,
});

const THEME_NAMES = new Set<string>(SYNTAX_THEMES);
const ACCENTS = new Set<string>(ACCENT_COLORS.map(({ value }) => value));

export function communityThemeStorageKey(
  pubkey: string,
  relayUrl: string,
): string {
  return `${STORAGE_KEY_PREFIX}:${pubkey}:${encodeURIComponent(normalizeRelayUrl(relayUrl))}`;
}

export function communityThemeOutboxKey(
  pubkey: string,
  relayUrl: string,
): string {
  return `${OUTBOX_KEY_PREFIX}:${pubkey}:${encodeURIComponent(normalizeRelayUrl(relayUrl))}`;
}

export function parseCommunityThemePreference(
  value: unknown,
): CommunityThemePreference | null {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return null;
  }
  const candidate = value as Record<string, unknown>;
  if (
    candidate.version !== 1 ||
    typeof candidate.theme !== "string" ||
    !THEME_NAMES.has(candidate.theme) ||
    typeof candidate.accent !== "string" ||
    !ACCENTS.has(candidate.accent) ||
    typeof candidate.followSystem !== "boolean"
  ) {
    return null;
  }
  return {
    version: 1,
    theme: candidate.theme as SyntaxThemeName,
    accent: candidate.accent,
    followSystem: candidate.followSystem,
  };
}

export function readCommunityThemePreference(
  pubkey: string,
  relayUrl: string,
): CommunityThemePreference | null {
  try {
    const raw = window.localStorage.getItem(
      communityThemeStorageKey(pubkey, relayUrl),
    );
    return raw ? parseCommunityThemePreference(JSON.parse(raw)) : null;
  } catch {
    return null;
  }
}

export function readCommunityThemeOutbox(
  pubkey: string,
  relayUrl: string,
): CommunityThemePreference | null {
  try {
    const raw = window.localStorage.getItem(
      communityThemeOutboxKey(pubkey, relayUrl),
    );
    return raw ? parseCommunityThemePreference(JSON.parse(raw)) : null;
  } catch {
    return null;
  }
}

export function writeCommunityThemeOutbox(
  pubkey: string,
  relayUrl: string,
  preference: CommunityThemePreference,
): boolean {
  try {
    window.localStorage.setItem(
      communityThemeOutboxKey(pubkey, relayUrl),
      JSON.stringify(preference),
    );
    return true;
  } catch {
    return false;
  }
}

export function clearCommunityThemeOutbox(
  pubkey: string,
  relayUrl: string,
  acknowledged: CommunityThemePreference,
): void {
  const pending = readCommunityThemeOutbox(pubkey, relayUrl);
  if (!pending || !sameCommunityThemePreference(pending, acknowledged)) return;
  try {
    window.localStorage.removeItem(communityThemeOutboxKey(pubkey, relayUrl));
  } catch {
    // A later retry can safely publish the same replaceable event again.
  }
}

export function hasMigratedCommunityTheme(pubkey: string): boolean {
  try {
    return (
      window.localStorage.getItem(`${MIGRATION_KEY_PREFIX}:${pubkey}`) ===
      "true"
    );
  } catch {
    return false;
  }
}

export function markCommunityThemeMigrated(pubkey: string): void {
  try {
    window.localStorage.setItem(`${MIGRATION_KEY_PREFIX}:${pubkey}`, "true");
  } catch {
    // The preference itself remains usable in memory when storage is full.
  }
}

export function writeCommunityThemePreference(
  pubkey: string,
  relayUrl: string,
  preference: CommunityThemePreference,
): boolean {
  try {
    window.localStorage.setItem(
      communityThemeStorageKey(pubkey, relayUrl),
      JSON.stringify(preference),
    );
    return true;
  } catch {
    return false;
  }
}

export function cacheAndApplyCommunityTheme(
  pubkey: string,
  relayUrl: string,
  preference: CommunityThemePreference,
  apply: (preference: CommunityThemePreference) => void,
): void {
  writeCommunityThemePreference(pubkey, relayUrl, preference);
  apply(preference);
}

export function communityThemeScopeFallback(
  migrated: boolean,
  inherited: CommunityThemePreference,
): CommunityThemePreference {
  return migrated ? DEFAULT_COMMUNITY_THEME : inherited;
}

export function appearanceModeOf(
  preference: CommunityThemePreference,
): DeviceAppearanceMode {
  if (preference.followSystem) return "system";
  return isLightTheme(preference.theme) ? "light" : "dark";
}

export function readDeviceAppearanceMode(): DeviceAppearanceMode | null {
  try {
    const raw = window.localStorage.getItem(DEVICE_APPEARANCE_MODE_KEY);
    return raw === "system" || raw === "light" || raw === "dark" ? raw : null;
  } catch {
    return null;
  }
}

export function writeDeviceAppearanceMode(mode: DeviceAppearanceMode): void {
  try {
    window.localStorage.setItem(DEVICE_APPEARANCE_MODE_KEY, mode);
  } catch {
    // The active appearance stays usable; the mode is re-derived next launch.
  }
}

/** Re-express a synced preference in this device's light/dark/system mode. */
export function withDeviceAppearanceMode(
  preference: CommunityThemePreference,
  mode: DeviceAppearanceMode,
): CommunityThemePreference {
  if (mode === "system") return { ...preference, followSystem: true };
  const wantLight = mode === "light";
  const theme =
    isLightTheme(preference.theme) === wantLight
      ? preference.theme
      : (getThemePair(preference.theme) ?? preference.theme);
  return { ...preference, theme, followSystem: false };
}

/** The light member of a paired theme, so both halves compare as one choice. */
function themeFamily(theme: SyntaxThemeName): SyntaxThemeName {
  return isLightTheme(theme) ? theme : (getThemePair(theme) ?? theme);
}

/**
 * Compares only the account-synced part of a preference (theme family and
 * accent). Light/dark/system is per device, so a mode-only change is neither
 * published nor treated as a different preference.
 */
export function sameCommunityThemePreference(
  left: CommunityThemePreference,
  right: CommunityThemePreference,
): boolean {
  return (
    themeFamily(left.theme) === themeFamily(right.theme) &&
    left.accent === right.accent
  );
}

export function communityThemeApplyExpectation(
  preference: CommunityThemePreference,
  current: CommunityThemePreference,
  preserveNoop = false,
): CommunityThemePreference | null {
  return preserveNoop || !sameCommunityThemePreference(preference, current)
    ? preference
    : null;
}

/**
 * Decide whether the current context value is safe to persist for this scope.
 * Applying a scoped preference updates the outer ThemeProvider asynchronously,
 * so renders that still expose the previous scope must be deferred.
 */
export function communityThemePersistenceAction(
  expectedApplied: CommunityThemePreference | null,
  current: CommunityThemePreference,
): "persist" | "defer" | "acknowledge" {
  if (!expectedApplied) return "persist";
  return sameCommunityThemePreference(expectedApplied, current)
    ? "acknowledge"
    : "defer";
}
