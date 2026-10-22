import { createIntl, createIntlCache, type IntlShape } from "react-intl";

import enJson from "@/locales/en.json";
import { SOURCE_LOCALE } from "./locales";

type EnglishFile = typeof enJson;

/** Every UI string id, from `src/locales/en.json` (notes excluded). */
export type MessageKey = Exclude<keyof EnglishFile, `@${string}`>;

/** Values for `{placeholders}`; functions render `<tags>` as React nodes. */
export type MessageValues = Parameters<IntlShape["formatMessage"]>[1];

/** Placeholder values that always produce a plain string. */
export type PlainValues = Record<
  string,
  string | number | boolean | Date | null | undefined
>;

/** English strings without the `@key` notes. */
export const englishMessages: Record<string, string> = Object.fromEntries(
  Object.entries(enJson).filter(([key]) => !key.startsWith("@")),
) as Record<string, string>;

const cache = createIntlCache();

export function buildIntl(
  locale: string,
  messages: Record<string, string>,
): IntlShape {
  return createIntl(
    {
      locale,
      defaultLocale: SOURCE_LOCALE,
      // English first, so a string missing from a language shows in English.
      messages: { ...englishMessages, ...messages },
      onError(error) {
        if (import.meta.env?.DEV) console.warn("[i18n]", error.message);
      },
    },
    cache,
  );
}

let current = buildIntl(SOURCE_LOCALE, {});
const listeners = new Set<() => void>();

/** The app-wide translator, also for code outside React (toasts, notifications). */
export function getIntl(): IntlShape {
  return current;
}

export function setIntl(intl: IntlShape): void {
  current = intl;
  for (const listener of listeners) listener();
}

export function subscribeIntl(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** Translates outside React. Inside components use `useT()` so text updates on language change. */
export function t(key: MessageKey, values?: PlainValues): string {
  return current.formatMessage({ id: key }, values);
}
