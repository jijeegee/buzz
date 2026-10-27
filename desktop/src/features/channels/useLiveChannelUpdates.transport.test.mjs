// Real hook -> RelayClient -> mocked Tauri IPC. No native app or relay traffic.
import assert from "node:assert/strict";
import { after, afterEach, beforeEach, test } from "node:test";
import { JSDOM } from "jsdom";

const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://localhost",
});
Object.assign(globalThis, {
  window: dom.window,
  document: dom.window.document,
  HTMLElement: dom.window.HTMLElement,
  localStorage: dom.window.localStorage,
  IS_REACT_ACT_ENVIRONMENT: true,
});
const writes = [];
const repairs = [];
let sendHook = async () => {};
window.__TAURI_INTERNALS__ = {
  async invoke(command, args) {
    if (command === "plugin:websocket|send") {
      writes.push(JSON.parse(args.message.data));
      return sendHook(JSON.parse(args.message.data));
    }
    if (command === "plugin:websocket|disconnect") return;
    if (command === "get_channel_reconnect_repair") {
      repairs.push(args);
      return [];
    }
    assert.fail(`unexpected IPC: ${command}`);
  },
};
// Advance the production drain and readiness timers on the same deterministic clock.
const originalNow = Date.now;
let now = 1_000_000;
Date.now = () => now;
const timers = new Map();
let timerId = 1;
window.setTimeout = (fn, ms, ...args) => {
  const id = timerId++;
  timers.set(id, { fn, args, at: now + ms });
  return id;
};
window.clearTimeout = (id) => timers.delete(id);
const React = await import("react");
const { act, cleanup, renderHook } = await import("@testing-library/react");
const { QueryClient, QueryClientProvider } = await import(
  "@tanstack/react-query"
);
const { relayClient } = await import("@/shared/api/relayClient");
const { useLiveChannelUpdates } = await import("./useLiveChannelUpdates.ts");
const clients = [];
const channels = (count) =>
  Array.from({ length: count }, (_, i) => ({
    id: `channel-${i}`,
    name: `channel-${i}`,
    channelType: "stream",
  }));
const frames = (op) => writes.filter((f) => f[0] === op);
const flush = async () => {
  for (let i = 0; i < 30; i++) await Promise.resolve();
};
const deliver = (frame) =>
  relayClient.handleWsMessage(
    { type: "Text", data: JSON.stringify(frame) },
    relayClient.connectionGeneration,
  );
beforeEach(() => {
  now = 1_000_000;
  writes.length = 0;
  repairs.length = 0;
  sendHook = async () => {};
  relayClient.wsId = 7;
});
afterEach(async () => {
  await act(async () => {
    cleanup();
    await flush();
  });
  relayClient.disconnect();
  for (const client of clients.splice(0)) client.clear();
  // EOSE can flush a preceding EVENT before its already-scheduled batch tick.
  await advance(50);
  assert.equal(timers.size, 0, "no retired readiness or drain timers");
});
after(() => {
  Date.now = originalNow;
  dom.window.close();
});
async function advance(ms) {
  await act(async () => {
    const target = now + ms;
    let fired = 0;
    for (;;) {
      const next = [...timers]
        .filter(([, t]) => t.at <= target)
        .sort((a, b) => a[1].at - b[1].at || a[0] - b[0])[0];
      if (!next) break;
      assert.ok(++fired < 1000, "timer loop must make progress");
      now = next[1].at;
      timers.delete(next[0]);
      next[1].fn(...next[1].args);
      await flush();
    }
    now = target;
    await flush();
  });
}
async function mount(count, strict = false, onChannelMessage = () => {}) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: Infinity } },
  });
  clients.push(queryClient);
  const wrapper = ({ children }) =>
    React.createElement(QueryClientProvider, { client: queryClient }, children);
  let hook;
  await act(async () => {
    hook = renderHook(
      ({ members }) =>
        useLiveChannelUpdates(members, null, {
          currentPubkey: "a".repeat(64),
          onChannelMessage,
        }),
      {
        wrapper,
        reactStrictMode: strict,
        initialProps: { members: channels(count) },
      },
    );
    await flush();
  });
  return {
    async members(members) {
      await act(async () => {
        hook.rerender({ members });
        await flush();
      });
    },
    async unmount() {
      await act(async () => {
        hook.unmount();
        await flush();
      });
    },
    async eose() {
      await act(async () => {
        for (const f of frames("REQ")) await deliver(["EOSE", f[1]]);
        await flush();
      });
    },
  };
}
const roomIds = (frame) => frame[2]["#h"];
for (const strict of [false, true]) {
  test(`154 channels ride two room-bundle REQs (StrictMode=${strict})`, async () => {
    const h = await mount(154, strict);
    try {
      assert.equal(frames("REQ").length, 1, "cold setup must not burst");
      await advance(250);
      assert.equal(frames("REQ").length, 2);
      assert.deepEqual(
        frames("REQ").map((f) => roomIds(f).length),
        [128, 26],
      );
      assert.equal(new Set(frames("REQ").flatMap(roomIds)).size, 154);
      for (const [, , filter] of frames("REQ")) {
        assert.equal(filter.limit, 0);
        assert.equal(filter.since, undefined);
        assert.ok(filter.kinds.includes(39005));
      }
    } finally {
      await h.eose();
    }
  });
}
test("a changed room set swaps bundles only after the successor is live, delivering once", async () => {
  const observed = [];
  const h = await mount(2, false, (...args) => observed.push(args));
  try {
    await h.eose();
    const [old] = frames("REQ");
    await h.members(channels(3));
    // The live REQ drain admits one REQ per 250 ms.
    await advance(250);
    assert.equal(frames("REQ").length, 2);
    const replacement = frames("REQ").at(-1);
    assert.deepEqual(roomIds(replacement), [
      "channel-0",
      "channel-1",
      "channel-2",
    ]);
    assert.equal(frames("CLOSE").length, 0, "old bundle covers meanwhile");
    const shared = {
      id: "shared",
      kind: 9,
      pubkey: "b".repeat(64),
      created_at: 1,
      content: "hi",
      tags: [["h", "channel-1"]],
      sig: "",
    };
    await act(async () => {
      await deliver(["EVENT", old[1], shared]);
      await deliver(["EVENT", replacement[1], shared]);
      await flush();
    });
    await advance(50);
    assert.equal(observed.length, 1, "overlap delivers the event once");
    assert.equal(observed[0][0], "channel-1");
    await h.eose();
    await advance(500);
    assert.deepEqual(
      frames("CLOSE").map((f) => f[1]),
      [old[1]],
      "old bundle closes once the successor is live",
    );
  } finally {
    await h.eose();
  }
});
test("an h-less event on a one-room bundle belongs to that room", async () => {
  const observed = [];
  const h = await mount(1, false, (...args) => observed.push(args));
  try {
    const [first] = frames("REQ");
    await act(async () => {
      await deliver([
        "EVENT",
        first[1],
        {
          id: "event",
          kind: 9,
          pubkey: "b".repeat(64),
          created_at: 1,
          content: "hi",
          tags: [],
          sig: "",
        },
      ]);
      await deliver(["EOSE", first[1]]);
      await flush();
    });
    await advance(50);
    assert.equal(observed.length, 1);
    assert.equal(observed[0][0], "channel-0");
  } finally {
    await h.eose();
  }
});
for (const stop of ["remove", "unmount", "workspace disconnect"]) {
  test(`${stop} while connection is pending cannot dispatch retired REQs`, async () => {
    const connection = Promise.withResolvers();
    relayClient.connectPromise = connection.promise;
    const h = await mount(2);
    assert.equal(frames("REQ").length, 0);
    try {
      if (stop === "remove") await h.members([]);
      if (stop === "unmount") await h.unmount();
      if (stop === "workspace disconnect") {
        relayClient.disconnect();
        relayClient.wsId = 8;
      }
      await act(async () => {
        connection.resolve(relayClient.connectionGeneration);
        await flush();
      });
      assert.equal(frames("REQ").length, 0);
    } finally {
      relayClient.connectPromise = null;
      await h.eose();
    }
  });
}

test("reconnect re-sends the bundle unchanged and repairs every room in one walk", async () => {
  const h = await mount(2);
  try {
    await h.eose();
    const [original] = frames("REQ");
    await act(async () => {
      relayClient.resetConnection(new Error("socket closed"));
      window.clearTimeout(relayClient.reconnectTimeout);
      relayClient.reconnectTimeout = null;
      relayClient.wsId = 8;
      await relayClient.replayLiveSubscriptions();
      await flush();
    });
    assert.deepEqual(frames("REQ"), [original, original]);
    assert.equal(repairs.length, 1, "one repair page for the whole bundle");
    assert.deepEqual(repairs[0].channelIds, ["channel-0", "channel-1"]);
    // Continuity started when the bundle subscribed, not at the first event.
    assert.equal(repairs[0].since, 0);
  } finally {
    await h.eose();
  }
});

test("late initial send failure after real replay retains the bundle and its original floor", async () => {
  const first = Promise.withResolvers();
  let requests = 0;
  sendHook = async (f) => {
    if (f[0] === "REQ" && ++requests === 1) await first.promise;
  };
  const h = await mount(1);
  try {
    const original = structuredClone(frames("REQ")[0]);
    const entry = relayClient.subscriptions.get(original[1]);
    await act(async () => {
      relayClient.resetConnection(new Error("socket closed"));
      window.clearTimeout(relayClient.reconnectTimeout);
      relayClient.reconnectTimeout = null;
      relayClient.wsId = 8;
      await relayClient.replayLiveSubscriptions();
      first.reject(new Error("late IPC failure"));
      await flush();
    });
    assert.equal(relayClient.subscriptions.get(original[1]), entry);
    assert.equal(frames("CLOSE").length, 0);
    assert.deepEqual(frames("REQ"), [original, original]);
    await h.members(channels(2));
    await advance(250);
    assert.equal(
      frames("REQ").length,
      3,
      "a changed room set opens one successor bundle",
    );
  } finally {
    first.resolve();
    await h.eose();
  }
});
