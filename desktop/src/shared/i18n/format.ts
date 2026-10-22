import { getIntl } from "./intl";

const PSEUDO_LOCALE = "en-XA";

/**
 * Locale for dates and numbers: the app language, not the operating system's,
 * so a Korean UI on an English Windows still says "10월 23일".
 */
export function formatLocale(): string {
  const locale = getIntl().locale;
  return locale === PSEUDO_LOCALE ? "en" : locale;
}

const cache = new Map<string, unknown>();

function cached<T>(
  kind: string,
  options: object | undefined,
  make: (locale: string) => T,
): T {
  const locale = formatLocale();
  const key = `${kind}|${locale}|${JSON.stringify(options ?? {})}`;
  let formatter = cache.get(key) as T | undefined;
  if (!formatter) {
    formatter = make(locale);
    cache.set(key, formatter);
  }
  return formatter;
}

/**
 * Drop-in for a module-level `new Intl.DateTimeFormat(...)`: same `format` /
 * `formatToParts`, but each call uses the current app language.
 */
export function localeDateTimeFormat(options?: Intl.DateTimeFormatOptions) {
  const get = () =>
    cached("dt", options, (locale) => new Intl.DateTimeFormat(locale, options));
  return {
    format: (date?: Date | number) => get().format(date),
    formatToParts: (date?: Date | number) => get().formatToParts(date),
  };
}

/** Drop-in for a module-level `new Intl.NumberFormat(...)`. */
export function localeNumberFormat(options?: Intl.NumberFormatOptions) {
  const get = () =>
    cached("num", options, (locale) => new Intl.NumberFormat(locale, options));
  return {
    format: (value: number | bigint) => get().format(value),
    formatToParts: (value: number | bigint) => get().formatToParts(value),
  };
}

/** Drop-in for a module-level `new Intl.RelativeTimeFormat(...)`. */
export function localeRelativeTimeFormat(
  options?: Intl.RelativeTimeFormatOptions,
) {
  const get = () =>
    cached(
      "rel",
      options,
      (locale) => new Intl.RelativeTimeFormat(locale, options),
    );
  return {
    format: (value: number, unit: Intl.RelativeTimeFormatUnit) =>
      get().format(value, unit),
  };
}
