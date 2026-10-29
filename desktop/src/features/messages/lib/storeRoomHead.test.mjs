import assert from "node:assert/strict";
import test from "node:test";

import { QueryClient } from "@tanstack/react-query";

import { channelMessagesKey } from "./messageQueryKeys.ts";
import {
  openChannelWindowEvents,
  setStoreRoomHeadDeps,
} from "./storeRoomHead.ts";

const stored = [{ id: "s1", kind: 9 }];
const relay = [{ id: "r1", kind: 9 }];

function harness({ liveAt = 100, head = stored } = {}) {
  const calls = { roomHead: [], relay: 0 };
  const restore = setStoreRoomHeadDeps({
    liveAt: () => liveAt,
    roomHead: async (roomId, at) => {
      calls.roomHead.push([roomId, at]);
      return head;
    },
    relayWindow: async () => {
      calls.relay += 1;
      return relay;
    },
  });
  return { calls, restore };
}

test("a first open takes the certain stored window", async () => {
  const { calls, restore } = harness();
  try {
    const events = await openChannelWindowEvents(new QueryClient(), "room");
    assert.deepEqual(events, stored);
    assert.deepEqual(calls.roomHead, [["room", 100]]);
    assert.equal(calls.relay, 0);
  } finally {
    restore();
  }
});

test("later fetches ask the relay", async () => {
  const { calls, restore } = harness();
  try {
    const queryClient = new QueryClient();
    queryClient.setQueryData(channelMessagesKey("room"), []);
    assert.deepEqual(await openChannelWindowEvents(queryClient, "room"), relay);
    assert.equal(calls.roomHead.length, 0);
  } finally {
    restore();
  }
});

test("no live REQ or no certain window asks the relay", async () => {
  for (const options of [{ liveAt: null }, { head: null }]) {
    const { calls, restore } = harness(options);
    try {
      assert.deepEqual(
        await openChannelWindowEvents(new QueryClient(), "room"),
        relay,
      );
      assert.equal(calls.relay, 1);
    } finally {
      restore();
    }
  }
});
