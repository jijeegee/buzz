import assert from "node:assert/strict";
import test from "node:test";

import { buildRelayAuthFrame, TOKEN_AUTH_OK_ID } from "./relayAuthFrame.ts";

const challenge = { challenge: "c1", relayUrl: "ws://relay" };

test("token mode sends the bearer frame and never signs", async () => {
  let signed = false;
  const frame = await buildRelayAuthFrame(challenge, {
    tokenFrame: async () => ({ token: "bzs_x" }),
    signChallenge: async () => {
      signed = true;
      return { id: "e1" };
    },
  });
  assert.deepEqual(frame, {
    pendingId: TOKEN_AUTH_OK_ID,
    payload: { token: "bzs_x" },
  });
  assert.equal(signed, false);
});

test("key mode signs the NIP-42 challenge and waits on its event id", async () => {
  const event = { id: "e1", kind: 22242 };
  const frame = await buildRelayAuthFrame(challenge, {
    tokenFrame: async () => null,
    signChallenge: async (input) => {
      assert.deepEqual(input, challenge);
      return event;
    },
  });
  assert.deepEqual(frame, { pendingId: "e1", payload: event });
});

test("a blocked token session fails instead of falling back to the key", async () => {
  await assert.rejects(
    buildRelayAuthFrame(challenge, {
      tokenFrame: async () => {
        throw new Error("restoring your Google sign-in");
      },
      signChallenge: async () => ({ id: "e1" }),
    }),
    /restoring/,
  );
});
