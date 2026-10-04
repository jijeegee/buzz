import assert from "node:assert/strict";
import test from "node:test";

import {
  buildWsAuthFrame,
  resolveHttpAuthorization,
  TOKEN_AUTH_OK_ID,
} from "./auth-mode.ts";
import { SignInRequiredError } from "./web-session.ts";

function sources({ tokenMode, token }) {
  const used = { nip98: 0, sign: 0 };
  return {
    used,
    tokenMode: async () => tokenMode,
    accessToken: async () => token,
    nip98: async () => {
      used.nip98 += 1;
      return "Nostr e30=";
    },
    signChallenge: async () => {
      used.sign += 1;
      return { id: "event-id", kind: 22242 };
    },
  };
}

test("token mode with a session uses Bearer and never signs", async () => {
  const s = sources({ tokenMode: true, token: "bzs_x" });
  assert.equal(await resolveHttpAuthorization(s), "Bearer bzs_x");
  assert.equal(s.used.nip98, 0);
});

test("key mode keeps NIP-98", async () => {
  const s = sources({ tokenMode: false, token: "bzs_x" });
  assert.equal(await resolveHttpAuthorization(s), "Nostr e30=");
  assert.equal(s.used.nip98, 1);
});

test("signed-out public reads fall back to NIP-98 in token mode", async () => {
  const s = sources({ tokenMode: true, token: null });
  assert.equal(await resolveHttpAuthorization(s), "Nostr e30=");
});

test("durable flows require the session instead of a throwaway key", async () => {
  const s = sources({ tokenMode: true, token: null });
  await assert.rejects(
    () => resolveHttpAuthorization(s, { requireSession: true }),
    SignInRequiredError,
  );
  assert.equal(s.used.nip98, 0);
});

test("WS: token mode answers the challenge with a token frame", async () => {
  const s = sources({ tokenMode: true, token: "bzs_x" });
  assert.deepEqual(await buildWsAuthFrame(s), {
    okId: TOKEN_AUTH_OK_ID,
    message: ["AUTH", { token: "bzs_x" }],
  });
  assert.equal(s.used.sign, 0);
});

test("WS: key mode or no session signs NIP-42 and waits on its id", async () => {
  for (const config of [
    { tokenMode: false, token: "bzs_x" },
    { tokenMode: true, token: null },
  ]) {
    const s = sources(config);
    const frame = await buildWsAuthFrame(s);
    assert.equal(frame.okId, "event-id");
    assert.deepEqual(frame.message, ["AUTH", { id: "event-id", kind: 22242 }]);
    assert.equal(s.used.sign, 1);
  }
});
