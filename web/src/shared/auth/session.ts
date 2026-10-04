/**
 * Browser wiring for the token-auth session: one [`WebSession`] per page,
 * token-mode detection cached per page, and the credential callbacks the
 * relay clients use.
 */

import { useEffect, useSyncExternalStore } from "react";

import { makeNip98AuthHeader } from "@/shared/lib/nip98";
import { relayHttpBaseUrl } from "@/shared/lib/relay-url";

import { type AuthSources, resolveHttpAuthorization } from "./auth-mode";
import { beginLogin } from "./pkce";
import {
  fetchTokenAuthDescriptor,
  type TokenAuthDescriptor,
} from "./token-mode";
import { type SessionSnapshot, WebSession } from "./web-session";

const browserFetch = (input: string, init?: RequestInit) =>
  window.fetch(input, init);

/** The page's session (access token in memory, refresh in the cookie). */
export const webSession = new WebSession({
  baseUrl: relayHttpBaseUrl(),
  fetch: browserFetch,
});

let descriptorPromise: Promise<TokenAuthDescriptor | null> | null = null;
let restoreStarted = false;

/** The relay's token-auth descriptor (`null` = key mode). Cached per page. */
export function tokenAuthDescriptor(): Promise<TokenAuthDescriptor | null> {
  if (!descriptorPromise) {
    descriptorPromise = fetchTokenAuthDescriptor(
      relayHttpBaseUrl(),
      browserFetch,
    ).then((descriptor) => {
      // Restore the cookie session once, and only on token-auth relays.
      if (descriptor && !restoreStarted) {
        restoreStarted = true;
        void webSession.restore();
      }
      return descriptor;
    });
  }
  return descriptorPromise;
}

/** Credential sources backed by this page's session. */
export const browserAuthSources: AuthSources = {
  tokenMode: async () => (await tokenAuthDescriptor()) !== null,
  accessToken: async () =>
    (await tokenAuthDescriptor()) ? webSession.getAccessToken() : null,
};

/** `Authorization` header for a relay request: Bearer in token mode, else NIP-98. */
export function relayAuthorization(
  url: string,
  method: string,
  options?: { body?: string; requireNip07?: boolean; requireSession?: boolean },
): Promise<string> {
  return resolveHttpAuthorization(
    {
      ...browserAuthSources,
      nip98: () =>
        makeNip98AuthHeader(url, method, {
          body: options?.body,
          requireNip07: options?.requireNip07,
        }),
    },
    { requireSession: options?.requireSession },
  );
}

/** Navigate to the relay's Google sign-in, returning to `returnTo` after. */
export async function startGoogleSignIn(returnTo?: string): Promise<void> {
  const url = await beginLogin({
    storage: window.sessionStorage,
    relayBaseUrl: relayHttpBaseUrl(),
    redirectOrigin: window.location.origin,
    returnTo:
      returnTo ?? `${window.location.pathname}${window.location.search}`,
  });
  window.location.assign(url);
}

/** React view of the session, plus whether the relay uses token auth. */
export function useWebSession(): SessionSnapshot & {
  tokenAuth: TokenAuthDescriptor | null | undefined;
} {
  const snapshot = useSyncExternalStore(
    webSession.subscribe,
    webSession.getSnapshot,
  );
  const tokenAuth = useTokenAuthDescriptor();
  useEffect(() => {
    if (snapshot.status === "signed_in" && snapshot.displayName === null) {
      void webSession.loadProfile();
    }
  }, [snapshot.status, snapshot.displayName]);
  return { ...snapshot, tokenAuth };
}

let resolvedDescriptor: TokenAuthDescriptor | null | undefined;
const descriptorListeners = new Set<() => void>();

function useTokenAuthDescriptor(): TokenAuthDescriptor | null | undefined {
  return useSyncExternalStore(
    (listener) => {
      descriptorListeners.add(listener);
      if (resolvedDescriptor === undefined) {
        void tokenAuthDescriptor().then((descriptor) => {
          resolvedDescriptor = descriptor;
          for (const notify of descriptorListeners) notify();
        });
      }
      return () => descriptorListeners.delete(listener);
    },
    () => resolvedDescriptor,
  );
}
