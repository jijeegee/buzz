import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import test from "node:test";

import {
  beginLogin,
  LoginStateMismatchError,
  PENDING_LOGIN_KEY,
  pkceChallenge,
  safeReturnTo,
  takePendingLogin,
} from "./pkce.ts";
import { parseTokenAuthDescriptor } from "./token-mode.ts";

function memoryStorage() {
  const map = new Map();
  return {
    getItem: (key) => (map.has(key) ? map.get(key) : null),
    setItem: (key, value) => map.set(key, String(value)),
    removeItem: (key) => map.delete(key),
  };
}

const relay = {
  relayBaseUrl: "https://relay.test/",
  redirectOrigin: "https://relay.test",
};

test("beginLogin stores the verifier and builds the web start URL", async () => {
  const storage = memoryStorage();
  const url = new URL(
    await beginLogin({ storage, ...relay, returnTo: "/invite/abc" }),
  );
  const pending = JSON.parse(storage.getItem(PENDING_LOGIN_KEY));
  assert.equal(
    url.origin + url.pathname,
    "https://relay.test/auth/oidc/google/start",
  );
  assert.equal(url.searchParams.get("client"), "web");
  assert.equal(
    url.searchParams.get("identity_mode"),
    "token",
    "web explicitly requests its supported legacy mode",
  );
  assert.equal(
    url.searchParams.get("redirect_uri"),
    "https://relay.test/auth/cb",
  );
  assert.equal(url.searchParams.get("state"), pending.state);
  assert.match(pending.state, /^[A-Za-z0-9_-]{16,256}$/);
  assert.equal(pending.verifier.length, 43);
  const expected = createHash("sha256")
    .update(pending.verifier)
    .digest("base64url");
  assert.equal(url.searchParams.get("code_challenge"), expected);
  assert.equal(await pkceChallenge(pending.verifier), expected);
  assert.equal(pending.returnTo, "/invite/abc");
});

test("a callback with another state is rejected and the login discarded", async () => {
  const storage = memoryStorage();
  await beginLogin({ storage, ...relay });
  assert.throws(
    () => takePendingLogin(storage, "attacker-state-0123456789"),
    LoginStateMismatchError,
  );
  assert.equal(
    storage.getItem(PENDING_LOGIN_KEY),
    null,
    "consumed even on mismatch",
  );
});

test("a matching callback yields the verifier exactly once", async () => {
  const storage = memoryStorage();
  await beginLogin({ storage, ...relay });
  const { state, verifier } = JSON.parse(storage.getItem(PENDING_LOGIN_KEY));
  assert.equal(takePendingLogin(storage, state).verifier, verifier);
  assert.throws(
    () => takePendingLogin(storage, state),
    LoginStateMismatchError,
  );
  assert.throws(() => takePendingLogin(storage, null), LoginStateMismatchError);
});

test("return paths stay same-origin", () => {
  assert.equal(safeReturnTo("/repos/x"), "/repos/x");
  assert.equal(safeReturnTo("//evil.test/x"), "/");
  assert.equal(safeReturnTo("https://evil.test"), "/");
  assert.equal(safeReturnTo("/auth/cb?code=1"), "/");
  assert.equal(safeReturnTo(undefined), "/");
  assert.equal(safeReturnTo("/\\evil.test/x"), "/", "backslash host");
  assert.equal(safeReturnTo("/repos\\x"), "/", "any backslash");
  assert.equal(safeReturnTo("/repos/\nx"), "/", "control characters");
  assert.equal(safeReturnTo("/./auth/cb"), "/", "normalized /auth path");
  assert.equal(safeReturnTo("/repos/x?tab=1#top"), "/repos/x?tab=1#top");
});

test("token mode is detected only from an advertised bearer descriptor", () => {
  assert.equal(parseTokenAuthDescriptor({ name: "relay" }), null);
  assert.equal(parseTokenAuthDescriptor(null), null);
  assert.equal(
    parseTokenAuthDescriptor({
      buzz_token_auth: { version: 1, bearer: false },
    }),
    null,
  );
  assert.deepEqual(
    parseTokenAuthDescriptor({
      buzz_token_auth: {
        version: 1,
        bearer: true,
        oidc_providers: ["google", 7],
      },
    }),
    { bearer: true, oidcProviders: ["google"] },
  );
});
