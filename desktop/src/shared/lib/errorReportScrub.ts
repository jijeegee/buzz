/**
 * Privacy filter for crash and error reports (see `errorReporting.ts`).
 *
 * Kept free of the Sentry SDK so it can be tested on its own. Mirrors the
 * Rust side in `src-tauri/src/error_reporting.rs`: reports keep the error,
 * where it happened, app version and OS only.
 */

/** Most reports one run may send; a crash loop must not use up the quota. */
export const MAX_REPORTS_PER_RUN = 20;
/** Longest error text kept, after scrubbing. */
export const MAX_TEXT_CHARS = 300;

const QUOTED = /`[^`]*`|"[^"]*"|'[^']{2,}'/g;
const URL_QUERY = /\b((?:https?|wss?):\/\/[^\s?#]+)[?#]\S*/gi;
const SECRETS = new RegExp(
  [
    String.raw`\b(?:nsec|npub|nprofile|nevent|naddr|note)1[0-9a-z]{10,}`,
    String.raw`[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}`,
    String.raw`\b[0-9a-f]{32,}\b`,
    String.raw`\b(?:bearer|token|key|secret|password|auth)\s*[:=]\s*\S+`,
    String.raw`[A-Za-z0-9_\-]{40,}`,
  ].join("|"),
  "gi",
);

/**
 * Scrubs one error string: quoted text (which can be message text), keys,
 * tokens, emails and URL queries are replaced, then cut to MAX_TEXT_CHARS.
 */
export function scrubText(text: string): string {
  const scrubbed = text
    .replace(QUOTED, "[quoted]")
    .replace(URL_QUERY, "$1")
    .replace(SECRETS, "[redacted]");
  const chars = Array.from(scrubbed);
  return chars.length <= MAX_TEXT_CHARS
    ? scrubbed
    : `${chars.slice(0, MAX_TEXT_CHARS).join("")}…`;
}

type ReportFrame = {
  function?: string;
  lineno?: number;
  vars?: unknown;
};

type ReportException = {
  type?: string;
  value?: string;
  stacktrace?: { frames?: ReportFrame[] };
};

/** The parts of a Sentry event this filter touches. */
export type ScrubbableEvent = {
  message?: string;
  user?: unknown;
  request?: unknown;
  server_name?: string;
  breadcrumbs?: unknown;
  extra?: unknown;
  tags?: unknown;
  exception?: { values?: ReportException[] };
};

/** Removes everything a report must not carry. */
export function scrubEvent<T extends ScrubbableEvent>(event: T): T {
  delete event.user;
  delete event.request;
  delete event.server_name;
  delete event.breadcrumbs;
  delete event.extra;
  delete event.tags;
  if (event.message !== undefined) event.message = scrubText(event.message);
  for (const exception of event.exception?.values ?? []) {
    if (exception.value !== undefined) {
      exception.value = scrubText(exception.value);
    }
    for (const frame of exception.stacktrace?.frames ?? []) {
      delete frame.vars;
    }
  }
  return event;
}

function reportKey(event: ScrubbableEvent): string {
  const parts = [event.message ?? ""];
  for (const exception of event.exception?.values ?? []) {
    const top = exception.stacktrace?.frames?.at(-1);
    parts.push(
      exception.type ?? "",
      exception.value ?? "",
      top?.function ?? "",
      String(top?.lineno ?? ""),
    );
  }
  return parts.join("\u0000");
}

/**
 * Admits each distinct error once per run, up to MAX_REPORTS_PER_RUN.
 * Call with already scrubbed events.
 */
export function createReportGate(): (event: ScrubbableEvent) => boolean {
  const seen = new Set<string>();
  return (event) => {
    if (seen.size >= MAX_REPORTS_PER_RUN) return false;
    const key = reportKey(event);
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  };
}
