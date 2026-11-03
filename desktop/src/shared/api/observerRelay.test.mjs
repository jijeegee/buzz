import assert from "node:assert/strict";
import test, { mock } from "node:test";

import { relayClient } from "./relayClient.ts";
import { meLive } from "./relayMeLive.ts";
import { subscribeToAgentObserverFrames } from "./observerRelay.ts";

// docs/message-sync.md#agent-activity: frames are never stored, so the
// observer asks for no history and listens on the addressed account bundle.
test("subscribeToAgentObserverFrames listens live-only through the account bundle", async () => {
  const calls = [];
  const stop = async () => {};
  mock.method(meLive, "subscribe", async (filter, onEvent) => {
    calls.push({ filter, onEvent });
    return stop;
  });
  mock.method(relayClient, "subscribeInteractive", () =>
    assert.fail("the observer must not open a REQ of its own"),
  );
  const onEvent = () => {};

  const result = await subscribeToAgentObserverFrames("owner-pubkey", onEvent);

  assert.equal(result, stop);
  assert.equal(calls.length, 1);
  assert.deepEqual(calls[0].filter, {
    kinds: [24200],
    "#p": ["owner-pubkey"],
    limit: 0,
  });
  assert.equal(calls[0].onEvent, onEvent);
  mock.reset();
});
