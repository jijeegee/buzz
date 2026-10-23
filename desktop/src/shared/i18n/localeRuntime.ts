import { buildIntl, setIntl } from "./intl";
import {
  LANGUAGES,
  languageDirection,
  matchLocale,
  SOURCE_LOCALE,
} from "./locales";
import {
  type LocalePreference,
  readLocalePreference,
  writeLocalePreference,
} from "./localePreference";

const PSEUDO_LOCALE = "en-XA";

// Each language file is its own chunk, loaded only when chosen. English is
// bundled with the app (intl.ts) and is always the fallback.
const files = import.meta.glob<{ default: Record<string, string> }>([
  "../../locales/*.json",
  "!../../locales/en.json",
]);
const fileByLocale = new Map(
  Object.entries(files).map(([path, load]) => [
    path.slice(path.lastIndexOf("/") + 1, -".json".length),
    load,
  ]),
);

/** Languages the user can pick: enabled in `i18n/languages.json` and shipped in this build. */
export function availableLocales(): string[] {
  const shipped = LANGUAGES.filter(
    (language) =>
      language.enabled &&
      (language.code === SOURCE_LOCALE || fileByLocale.has(language.code)),
  ).map((language) => language.code);
  return import.meta.env?.DEV && fileByLocale.has(PSEUDO_LOCALE)
    ? [...shipped, PSEUDO_LOCALE]
    : shipped;
}

export function resolveLocale(preference: LocalePreference): string {
  const available = availableLocales();
  if (preference !== "system" && available.includes(preference)) {
    return preference;
  }
  return matchLocale(globalThis.navigator?.languages ?? [], available);
}

async function loadMessages(locale: string): Promise<Record<string, string>> {
  if (locale === SOURCE_LOCALE) return {};
  const load = fileByLocale.get(locale);
  if (!load) return {};
  const raw = (await load()).default;
  return Object.fromEntries(
    Object.entries(raw).filter(([key]) => !key.startsWith("@")),
  );
}

/** Switches the whole app (React tree, `t()`, `<html lang dir>`) to a language. */
export async function applyLocale(locale: string): Promise<void> {
  let messages: Record<string, string> = {};
  try {
    messages = await loadMessages(locale);
  } catch (error) {
    console.error(`[i18n] could not load ${locale}; using English`, error);
    locale = SOURCE_LOCALE;
  }
  setIntl(buildIntl(locale, messages));
  const root = globalThis.document?.documentElement;
  root?.setAttribute("lang", locale);
  root?.setAttribute("dir", languageDirection(locale));
}

/** Runs before the first render so the app never flashes English first. */
export async function initializeLocale(): Promise<void> {
  await applyLocale(resolveLocale(readLocalePreference()));
}

export async function setLocalePreference(
  preference: LocalePreference,
): Promise<void> {
  writeLocalePreference(preference);
  await applyLocale(resolveLocale(preference));
}
