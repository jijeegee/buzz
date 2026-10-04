/**
 * Token-auth detection (plan §5.2.1): a relay with `AUTH_TOKEN_ENABLED`
 * advertises `buzz_token_auth: {version, bearer, oidc_providers}` in its
 * NIP-11 document. Without it (flag off, older relay, or an unreachable
 * document) the web client keeps today's key/NIP-98 behavior.
 */

/** Parsed `buzz_token_auth` descriptor. */
export type TokenAuthDescriptor = {
  bearer: boolean;
  oidcProviders: string[];
};

/** Parse a NIP-11 document; `null` when token auth is not advertised. */
export function parseTokenAuthDescriptor(
  nip11: unknown,
): TokenAuthDescriptor | null {
  if (!nip11 || typeof nip11 !== "object") return null;
  const raw = (nip11 as Record<string, unknown>).buzz_token_auth;
  if (!raw || typeof raw !== "object") return null;
  const descriptor = raw as Record<string, unknown>;
  if (descriptor.bearer !== true) return null;
  const providers = Array.isArray(descriptor.oidc_providers)
    ? descriptor.oidc_providers.filter(
        (name): name is string => typeof name === "string",
      )
    : [];
  return { bearer: true, oidcProviders: providers };
}

/** Fetch and parse the relay's NIP-11 document. Never throws. */
export async function fetchTokenAuthDescriptor(
  baseUrl: string,
  fetchImpl: (input: string, init?: RequestInit) => Promise<Response>,
): Promise<TokenAuthDescriptor | null> {
  try {
    const response = await fetchImpl(baseUrl, {
      headers: { Accept: "application/nostr+json" },
    });
    if (!response.ok) return null;
    return parseTokenAuthDescriptor(await response.json());
  } catch {
    return null;
  }
}
