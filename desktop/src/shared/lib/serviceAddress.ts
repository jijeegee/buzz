/**
 * Stored settings belong to the service, not to a server address.
 *
 * Kuzz is one service whose address can change (port, domain, a server
 * move). Settings stored for the built-in server are keyed by the service
 * scope (`kuzz`) instead of the address, so an address change keeps them; the
 * address is only used to connect. Any other address (a dev override, tests,
 * E2E mock builds without a built-in server) keeps its address key as before.
 *
 * The service's addresses are the build's server plus its earlier addresses
 * (build env `KUZZ_PREVIOUS_RELAY_URLS`, read from the native side). Keys an
 * older build wrote under an address are copied to the scope once by
 * {@link migrateServiceStorage}; the old keys are kept.
 */

type ServiceConfig = {
  scope: string;
  /** The built-in server's relay URL. */
  current: string;
  /** Canonical origins of the current and every earlier address. */
  origins: Set<string>;
  /** The current and earlier relay URLs as configured. */
  addresses: string[];
};

let config: ServiceConfig | null = null;

/**
 * `scheme://host[:port]` (default port dropped, lowercase), `ws`/`wss`
 * folded to `http`/`https`. A bare host means `wss`. `null` if unparseable.
 */
export function serviceOrigin(url: string | null | undefined): string | null {
  const trimmed = url?.trim();
  if (!trimmed) return null;
  const withScheme = /^[a-z][a-z0-9+.-]*:\/\//i.test(trimmed)
    ? trimmed
    : `wss://${trimmed}`;
  try {
    const parsed = new URL(withScheme.replace(/^ws(s?):\/\//i, "http$1://"));
    if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
      return null;
    }
    return parsed.host ? parsed.origin.toLowerCase() : null;
  } catch {
    return null;
  }
}

/** Set once the built-in server is known (`loadBuiltInServer`). */
export function configureService(
  next: { scope: string; current: string; previous: readonly string[] } | null,
): void {
  if (!next) {
    config = null;
    return;
  }
  const addresses = [next.current, ...next.previous]
    .map((url) => url.trim())
    .filter((url) => url.length > 0);
  const origins = new Set<string>();
  for (const url of addresses) {
    const origin = serviceOrigin(url);
    if (origin) origins.add(origin);
  }
  config = { scope: next.scope, current: next.current, origins, addresses };
}

/** Whether `url` (relay URL or origin) is one of the service's addresses. */
export function isServiceAddress(url: string | null | undefined): boolean {
  if (!config) return false;
  const origin = serviceOrigin(url);
  return origin !== null && config.origins.has(origin);
}

/**
 * The storage-key part for `relayUrl`: the service scope for the service's
 * addresses, otherwise `fallback` (the key part the caller used before).
 */
export function relayStorageScope(
  relayUrl: string | null | undefined,
  fallback: string,
): string {
  return config && isServiceAddress(relayUrl) ? config.scope : fallback;
}

/**
 * `url` on the current address when it points at one of the service's
 * addresses (path and query kept, `ws`/`http` kind kept); otherwise `url`
 * unchanged. Media links already signed into events keep the address they
 * were uploaded to.
 */
export function currentServiceUrl(url: string): string {
  if (!config || !isServiceAddress(url)) return url;
  try {
    const target = new URL(
      config.current.replace(/^ws(s?):\/\//i, "http$1://"),
    );
    const parsed = new URL(url);
    parsed.hostname = target.hostname;
    parsed.port = target.port;
    return parsed.toString();
  } catch {
    return url;
  }
}

/** localStorage key recording which old addresses were already moved. */
export const SERVICE_MIGRATION_KEY = "kuzz.service-scope-migrated.v1";

/**
 * The strings an older build wrote into storage keys for `url`: the relay
 * URL as configured and normalised (with and without `wss://`), its HTTP
 * origin, and each of those URI-encoded.
 */
function addressForms(url: string): string[] {
  const forms = new Set<string>();
  const trimmed = url.trim().replace(/\/+$/, "");
  const lower = trimmed.toLowerCase();
  const withScheme = /^wss?:\/\//i.test(lower) ? lower : `wss://${lower}`;
  const httpOrigin = withScheme.replace(/^ws(s?):\/\//, "http$1://");
  for (const form of [trimmed, lower, withScheme, httpOrigin]) {
    forms.add(form);
    forms.add(encodeURIComponent(form));
  }
  return [...forms];
}

/**
 * Copy every localStorage entry whose key contains one of the service's
 * address forms to the same key with the scope instead, unless that key is
 * already set. Each address is moved once (recorded under
 * {@link SERVICE_MIGRATION_KEY}), so a setting cleared after the move is
 * never brought back. Old keys are kept (an older app still reads them).
 * Returns how many entries were copied.
 */
export function migrateServiceStorage(storage: Storage = localStorage): number {
  if (!config) return 0;
  let moved: string[] = [];
  try {
    const raw = storage.getItem(SERVICE_MIGRATION_KEY);
    const parsed: unknown = raw ? JSON.parse(raw) : [];
    if (Array.isArray(parsed)) {
      moved = parsed.filter(
        (entry): entry is string => typeof entry === "string",
      );
    }
  } catch {
    moved = [];
  }
  const pending = config.addresses.filter(
    (url) => !moved.includes(serviceOrigin(url) ?? url),
  );
  if (pending.length === 0) return 0;

  // Longest first, so `host:8443` is never read as `host` followed by `:8443`.
  const forms = [...new Set(pending.flatMap(addressForms))].sort(
    (a, b) => b.length - a.length,
  );
  // Each form must end the address. After a port only another digit would
  // continue it; a form without a port must not be followed by more host
  // characters or by a port (1-5 digits ending the authority). So
  // `wss://host` never matches inside `wss://host:8443`, while
  // `host:8443:3fa9…` (a pubkey) still does.
  const portEnd = "(?![0-9])";
  const hostEnd = String.raw`(?![A-Za-z0-9.\-]|(?::|%3A)\d{1,5}(?![0-9A-Za-z]))`;
  const escaped = forms.map((form) => {
    const literal = form.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    return literal + (/(?::|%3A)\d+$/i.test(form) ? portEnd : hostEnd);
  });
  const pattern = new RegExp(escaped.join("|"), "g");

  const keys: string[] = [];
  for (let index = 0; index < storage.length; index++) {
    const key = storage.key(index);
    if (key !== null && key !== SERVICE_MIGRATION_KEY) keys.push(key);
  }
  let copied = 0;
  try {
    for (const key of keys) {
      pattern.lastIndex = 0;
      if (!pattern.test(key)) continue;
      pattern.lastIndex = 0;
      const target = key.replace(pattern, config.scope);
      if (target === key || storage.getItem(target) !== null) continue;
      const value = storage.getItem(key);
      if (value === null) continue;
      storage.setItem(target, value);
      copied++;
    }
  } catch (error) {
    // Quota or storage failure: leave the addresses unmarked so the next
    // launch tries again (already-copied keys are skipped then).
    console.error("[service] moving stored settings failed:", error);
    return copied;
  }
  const done = [...moved, ...pending.map((url) => serviceOrigin(url) ?? url)];
  try {
    storage.setItem(SERVICE_MIGRATION_KEY, JSON.stringify([...new Set(done)]));
  } catch {
    // Retried next launch.
  }
  return copied;
}

/** Test seam. */
export function resetServiceForTests(): void {
  config = null;
}
