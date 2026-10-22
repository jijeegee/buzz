/** "system" follows the device language; anything else is a language code. */
export type LocalePreference = "system" | (string & {});

export const LOCALE_STORAGE_KEY = "kuzz.locale";

export function readLocalePreference(): LocalePreference {
  try {
    return globalThis.localStorage?.getItem(LOCALE_STORAGE_KEY) || "system";
  } catch {
    return "system";
  }
}

export function writeLocalePreference(preference: LocalePreference): void {
  try {
    if (preference === "system") {
      globalThis.localStorage?.removeItem(LOCALE_STORAGE_KEY);
    } else {
      globalThis.localStorage?.setItem(LOCALE_STORAGE_KEY, preference);
    }
  } catch {
    // Storage can be unavailable (quota, WebKit SecurityError); keep the session choice only.
  }
}
