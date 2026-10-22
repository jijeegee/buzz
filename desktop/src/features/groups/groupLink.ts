/**
 * What a pasted or opened group link points at. Accepted forms:
 *
 * - `https://<host>/g/<code>` and `https://<host>/g/@<handle>`
 * - `kuzz://join/<code>` and `kuzz://join/@<handle>` (also this build's own
 *   demo scheme, e.g. `kuzz-demo-<slug>://join/...`)
 * - a bare invite code, or `@handle`
 *
 * Shapes match the server: codes are 22-64 URL-safe characters, handles 3-32
 * of `a-z0-9_` (case-insensitive, stored lowercase).
 */
import { t } from "@/shared/i18n";

export type GroupLinkTarget =
  | { kind: "code"; code: string }
  | { kind: "handle"; handle: string };

const CODE_RE = /^[A-Za-z0-9_-]{22,64}$/;
const HANDLE_RE = /^[a-z0-9_]{3,32}$/;
/** `kuzz` or a build-specific `<app>-demo-<slug>` deep-link scheme. */
const APP_SCHEME_RE = /^(kuzz|[a-z][a-z0-9]*-demo-[a-z0-9-]+)$/;

function targetFromSegment(raw: string): GroupLinkTarget | null {
  let value: string;
  try {
    value = decodeURIComponent(raw);
  } catch {
    return null;
  }
  if (value.startsWith("@")) {
    const handle = value.slice(1).toLowerCase();
    return HANDLE_RE.test(handle) ? { kind: "handle", handle } : null;
  }
  return CODE_RE.test(value) ? { kind: "code", code: value } : null;
}

/** Parse a group link or bare code/handle; null when it is none of them. */
export function parseGroupLink(input: string): GroupLinkTarget | null {
  const text = input.trim();
  if (!text) return null;

  const schemeMatch = /^([A-Za-z][A-Za-z0-9+.-]*):\/\/(.*)$/.exec(text);
  if (!schemeMatch) {
    // Bare forms: `@handle`, or a code. No slashes or spaces allowed.
    return /[\s/?#]/.test(text) ? null : targetFromSegment(text);
  }

  const scheme = schemeMatch[1].toLowerCase();
  // Group links carry no query or fragment; a trailing slash is fine.
  const rest = schemeMatch[2];
  if (/[?#\s]/.test(rest)) return null;
  const path = rest.replace(/\/+$/, "");
  const parts = path.split("/");

  if (scheme === "https" || scheme === "http") {
    // parts: [host, "g", target]
    if (parts.length !== 3 || !parts[0] || parts[1] !== "g") return null;
    if (parts[0].includes("@")) return null; // no user-info hosts
    return targetFromSegment(parts[2]);
  }
  if (APP_SCHEME_RE.test(scheme)) {
    // parts: ["join", target]
    if (parts.length !== 2 || parts[0].toLowerCase() !== "join") return null;
    return targetFromSegment(parts[1]);
  }
  return null;
}

/** `<host>/g/@` for the public address field, from the community URL. */
export function publicAddressPrefix(
  relayUrl: string | null | undefined,
): string {
  if (!relayUrl) return "/g/@";
  try {
    return `${new URL(relayUrl).host}/g/@`;
  } catch {
    return "/g/@";
  }
}

/**
 * Client-side check for a public address before saving; mirrors the server
 * (which also rejects reserved words, reported back as an error).
 */
export function normalizeGroupHandle(input: string): string {
  return input.trim().replace(/^@/, "").toLowerCase();
}

export function isValidGroupHandle(handle: string): boolean {
  return HANDLE_RE.test(handle);
}

export function publicAddressTakenText(): string {
  return t("groups.publicAddress.taken");
}

export function publicAddressInvalidText(): string {
  return t("groups.publicAddress.invalid");
}

export function publicAddressReservedText(): string {
  return t("groups.publicAddress.reserved");
}

/**
 * The message for a failed public-address save: 409 is taken; a 400 that
 * mentions "reserved" is reserved; any other 400 is the shape rule.
 */
export function publicAddressErrorText(
  status: number | null,
  serverText: string,
): string | null {
  if (status === 409) return publicAddressTakenText();
  if (status === 400) {
    return /reserved/i.test(serverText)
      ? publicAddressReservedText()
      : publicAddressInvalidText();
  }
  return null;
}
