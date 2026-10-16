/**
 * Messages written before the product rename carry app links under the old
 * scheme. Rewrite them to `kuzz://` so every link parser keeps opening them.
 */
const LEGACY_SCHEME = "buzz://"; // kuzz-rename: keep
const LEGACY_APP_LINK = /(^|[^\w.+/-])buzz:\/\//g; // kuzz-rename: keep

export function upgradeLegacyAppLinks(text: string): string {
  if (!text.includes(LEGACY_SCHEME)) return text;
  return text.replace(LEGACY_APP_LINK, "$1kuzz://");
}

export function isLegacyAppLinkProtocol(protocol: string): boolean {
  return protocol === "buzz:"; // kuzz-rename: keep
}
