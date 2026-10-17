import assert from "node:assert/strict";
import test from "node:test";

import { sendThreadMarker } from "./threadMarkerSend.ts";

const ROOT = "a".repeat(64);
const SCOPE = { relayUrl: "wss://relay.example", signerPubkey: "AB12" };

function recordingSend() {
  const calls = [];
  const send = async (...args) => {
    calls.push(args);
    return { eventId: "e1", createdAt: 1 };
  };
  return { calls, send };
}

test("close and reopen markers carry the captured account checks", async () => {
  const { calls, send } = recordingSend();
  for (const marker of ["closed", "reopened"]) {
    await sendThreadMarker(send, {
      channelId: "chan",
      marker,
      rootId: ROOT,
      scope: SCOPE,
    });
  }
  for (const [index, marker] of ["closed", "reopened"].entries()) {
    const args = calls[index];
    assert.equal(args[0], "chan");
    assert.equal(args[2], ROOT, "parent is the root");
    // Same positions the Projects sends use: expectedRelayUrl, expectedSignerPubkey.
    assert.equal(args[10], "wss://relay.example");
    assert.equal(args[11], "ab12");
    assert.equal(args[12], ROOT, "root event id");
    assert.equal(args[14], marker);
  }
  assert.equal(calls[0][1], "✅ Thread closed.");
  assert.equal(calls[1][1], "Thread reopened.");
});

test("Undo reuses the scope captured at Close, not the account active later", async () => {
  const { calls, send } = recordingSend();
  const capturedAtClose = { ...SCOPE };
  await sendThreadMarker(send, {
    channelId: "chan",
    marker: "closed",
    rootId: ROOT,
    scope: capturedAtClose,
  });
  // The user switches accounts; Undo still sends the original binding, which
  // the backend rejects against the new active account.
  await sendThreadMarker(send, {
    channelId: "chan",
    marker: "reopened",
    rootId: ROOT,
    scope: capturedAtClose,
  });
  assert.equal(calls[1][10], "wss://relay.example");
  assert.equal(calls[1][11], "ab12");
});

test("without a full account scope nothing is sent", async () => {
  const { calls, send } = recordingSend();
  for (const scope of [
    { relayUrl: null, signerPubkey: "ab" },
    { relayUrl: "wss://relay.example", signerPubkey: undefined },
    { relayUrl: " ", signerPubkey: " " },
  ]) {
    await assert.rejects(
      sendThreadMarker(send, {
        channelId: "chan",
        marker: "closed",
        rootId: ROOT,
        scope,
      }),
    );
  }
  assert.equal(calls.length, 0);
});
