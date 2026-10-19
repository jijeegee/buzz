import { relayStorageScope } from "@/shared/lib/serviceAddress";

/**
 * Normalizes a relay URL for use in storage keys.
 * Trim, strip trailing slashes, lowercase — ensures equivalent URLs map to
 * the same key regardless of formatting differences. Any address of the
 * built-in service maps to the service scope, so its settings survive an
 * address change (see `serviceAddress`).
 */
export function normalizeRelayUrl(relayUrl: string): string {
  return relayStorageScope(
    relayUrl,
    relayUrl.trim().replace(/\/+$/, "").toLowerCase(),
  );
}
