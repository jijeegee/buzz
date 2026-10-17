import assert from "node:assert/strict";
import test from "node:test";

import {
  chatStateCapabilityKey,
  getChatStateCapability,
  probeChatStateCapability,
  resetChatStateCapabilities,
  setChatStateCapability,
} from "./chatStateCapability.ts";

test("capability is per relay + pubkey and unknown until probed", async () => {
  resetChatStateCapabilities();
  const key = chatStateCapabilityKey("wss://relay", "ABC");
  assert.equal(key, "wss://relay|abc");
  assert.equal(chatStateCapabilityKey(undefined, "abc"), null);
  assert.equal(getChatStateCapability(key), "unknown");
  assert.equal(getChatStateCapability(null), "unknown");

  assert.equal(await probeChatStateCapability(key, async () => ({})), "on");
  assert.equal(
    getChatStateCapability(chatStateCapabilityKey("wss://relay", "other")),
    "unknown",
  );
  assert.equal(
    await probeChatStateCapability(key, async () => {
      throw new Error("chat_state_unavailable");
    }),
    "off",
  );
  // Offline or timeout: keep what we knew.
  assert.equal(
    await probeChatStateCapability(key, async () => {
      throw new Error("relay unreachable: request timed out");
    }),
    "off",
  );
});

test("a probe that lands after a newer answer is ignored", async () => {
  resetChatStateCapabilities();
  const key = "wss://relay|me";
  let resolveSlow;
  const slow = probeChatStateCapability(
    key,
    () =>
      new Promise((resolve) => {
        resolveSlow = resolve;
      }),
  );
  setChatStateCapability(key, "off");
  resolveSlow({});
  assert.equal(await slow, "off");

  let rejectOld;
  const old = probeChatStateCapability(
    key,
    () =>
      new Promise((_, reject) => {
        rejectOld = reject;
      }),
  );
  await probeChatStateCapability(key, async () => ({}));
  rejectOld(new Error("chat_state_unavailable"));
  assert.equal(await old, "on");
});
