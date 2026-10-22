import languagesJson from "@i18n-languages";

/** One entry of the shared `i18n/languages.json` list. */
export type Language = {
  code: string;
  name: string;
  english: string;
  dir: "ltr" | "rtl";
  enabled: boolean;
};

export const SOURCE_LOCALE = languagesJson.source;
export const LANGUAGES: readonly Language[] =
  languagesJson.languages as Language[];

// Regional and legacy tags that map onto one of our language codes.
const ALIASES: Record<string, string> = {
  "zh-cn": "zh-Hans",
  "zh-sg": "zh-Hans",
  "zh-my": "zh-Hans",
  "zh-tw": "zh-Hant",
  "zh-hk": "zh-Hant",
  "zh-mo": "zh-Hant",
  zh: "zh-Hans",
  pt: "pt-BR",
  tl: "fil",
  in: "id",
};

/**
 * Picks the first of the user's preferred languages (most preferred first,
 * like `navigator.languages`) that is one of `available`, else English.
 * `zh-Hant-TW` → `zh-Hant`, `es-MX` → `es`, `pt-PT` → `pt-BR`.
 */
export function matchLocale(
  preferred: readonly string[],
  available: readonly string[],
): string {
  const byLower = new Map(available.map((code) => [code.toLowerCase(), code]));
  for (const tag of preferred) {
    const parts = tag.replaceAll("_", "-").toLowerCase().split("-");
    const candidates = [parts.join("-"), parts.slice(0, 2).join("-"), parts[0]];
    for (const candidate of candidates) {
      const code =
        byLower.get(candidate) ??
        byLower.get(ALIASES[candidate]?.toLowerCase());
      if (code) return code;
    }
  }
  return SOURCE_LOCALE;
}

export function languageDirection(code: string): "ltr" | "rtl" {
  return LANGUAGES.find((language) => language.code === code)?.dir ?? "ltr";
}
