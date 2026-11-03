/**
 * useDmResurfaceFromMessages over the room bundle's quiet rooms.
 *
 * Hidden DMs ride the shared room chunks (no REQ of their own, no `since`,
 * no history); their messages reach the resurface coordinator, never the
 * all-rooms listeners. These tests mount the hook against the real
 * `relayClient.liveRooms` with only the socket-facing `subscribeLive` faked.
 */

import assert from "node:assert/strict";
import { after, afterEach, before, test } from "node:test";

import { JSDOM } from "jsdom";

const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://localhost",
});

const RELAY_URL = "wss://relay.example";
const VIEWER = "a".repeat(64);
const PEER = "b".repeat(64);

const tauriInternals = {
  invoke: (cmd) => {
    if (cmd === "get_channel_members") {
      return Promise.resolve({
        members: [
          { pubkey: VIEWER, role: "member", is_agent: false },
          { pubkey: PEER, role: "member", is_agent: false },
        ],
      });
    }
    return Promise.reject(new Error(`unmocked Tauri command: ${cmd}`));
  },
  transformCallback: () => Math.random(),
};

before(() => {
  Object.assign(globalThis, {
    document: dom.window.document,
    HTMLElement: dom.window.HTMLElement,
    IS_REACT_ACT_ENVIRONMENT: true,
    window: dom.window,
    localStorage: dom.window.localStorage,
  });
  globalThis.__TAURI_INTERNALS__ = tauriInternals;
  dom.window.__TAURI_INTERNALS__ = tauriInternals;
});

afterEach(() => {
  delete globalThis.isTauri;
  delete tauriInternals.metadata;
});

after(() => dom.window.close());

function seedCommunity() {
  window.localStorage.setItem(
    "kuzz-communities",
    JSON.stringify([
      {
        id: "community-a",
        name: "Community A",
        relayUrl: RELAY_URL,
        addedAt: "2026-01-01T00:00:00Z",
      },
    ]),
  );
  window.localStorage.setItem("kuzz-active-community-id", "community-a");
}

const dmId = (index) => `dm-${String(index).padStart(4, "0")}`;

function hiddenDmSnapshot(count) {
  return {
    id: "snapshot-1",
    kind: 30622,
    pubkey: VIEWER,
    content: "",
    created_at: 1,
    tags: Array.from({ length: count }, (_, index) => ["h", dmId(index)]),
    sig: "",
  };
}

function message(id, roomId, overrides = {}) {
  return {
    id,
    kind: 9,
    pubkey: PEER,
    content: "hi",
    created_at: 2,
    tags: [["h", roomId]],
    sig: "",
    ...overrides,
  };
}

async function mount(hiddenCount, reopen) {
  const { act, cleanup, renderHook } = await import("@testing-library/react");
  const React = (await import("react")).default;
  const { QueryClient, QueryClientProvider } = await import(
    "@tanstack/react-query"
  );
  const { CommunitiesProvider } = await import(
    "@/features/communities/useCommunities.tsx"
  );
  const { relayClient } = await import("@/shared/api/relayClient");
  const { useDmResurfaceFromMessages } = await import(
    "./useDmResurfaceFromMessages.ts"
  );

  const originalFetchEvents = relayClient.fetchEvents;
  const originalSubscribeLive = relayClient.subscribeLive;
  const subscriptions = [];
  relayClient.fetchEvents = async () =>
    hiddenCount > 0 ? [hiddenDmSnapshot(hiddenCount)] : [];
  relayClient.subscribeLive = async (filter, onEvent) => {
    const sub = { filter, onEvent, disposed: false };
    subscriptions.push(sub);
    return async () => {
      sub.disposed = true;
    };
  };

  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  const wrapper = ({ children }) =>
    React.createElement(
      QueryClientProvider,
      { client: queryClient },
      React.createElement(CommunitiesProvider, null, children),
    );
  const hook = renderHook(
    () =>
      useDmResurfaceFromMessages({
        pubkey: VIEWER,
        relayUrl: RELAY_URL,
        reopen: reopen ?? (async () => ({ id: "x" })),
      }),
    { wrapper },
  );

  const live = () => subscriptions.filter((sub) => !sub.disposed);
  return {
    act,
    relayClient,
    subscriptions,
    live,
    chunkOf: (roomId) =>
      live().find((sub) => sub.filter["#h"]?.includes(roomId)),
    async settle() {
      for (let i = 0; i < 6; i++) {
        await act(async () => {
          await new Promise((r) => setTimeout(r, 5));
        });
      }
    },
    async deliver(sub, event) {
      await act(async () => {
        sub.onEvent(event);
        await new Promise((r) => setTimeout(r, 5));
      });
    },
    unmount: hook.unmount,
    restore() {
      hook.unmount();
      queryClient.clear();
      queryClient.unmount();
      cleanup();
      // The bundle lives on the singleton client: forget this test's rooms.
      relayClient.disconnect();
      relayClient.fetchEvents = originalFetchEvents;
      relayClient.subscribeLive = originalSubscribeLive;
    },
  };
}

test("129 hidden DMs ride the room bundle: no REQ of their own, no since, no history", async () => {
  seedCommunity();
  const h = await mount(129);
  try {
    await h.settle();
    const { ROOM_LIVE_KINDS } = await import("@/shared/api/relayRoomBundle.ts");
    const chunks = h.live();
    assert.deepEqual(
      chunks.map((sub) => sub.filter["#h"].length),
      [128, 1],
    );
    for (const sub of chunks) {
      assert.deepEqual(sub.filter.kinds, [...ROOM_LIVE_KINDS]);
      assert.equal(sub.filter.limit, 0);
      assert.equal(sub.filter.since, undefined);
    }
    assert.equal(new Set(chunks.flatMap((sub) => sub.filter["#h"])).size, 129);
  } finally {
    h.restore();
  }
});

test("a message in a hidden DM resurfaces it once; other kinds do not", async () => {
  seedCommunity();
  const reopened = [];
  const target = dmId(128);
  const h = await mount(129, async ({ pubkeys }) => {
    reopened.push(pubkeys);
    return { id: target };
  });
  try {
    await h.settle();
    const chunk = h.chunkOf(target);
    await h.deliver(chunk, message("system", target, { kind: 40099 }));
    await h.deliver(
      chunk,
      message("like", target, { kind: 7, tags: [["h", target]] }),
    );
    assert.equal(reopened.length, 0);
    await h.deliver(chunk, message("evt-1", target));
    // Reconnect repair or an overlapping chunk carries it again.
    await h.deliver(chunk, message("evt-1", target));
    assert.deepEqual(reopened, [[PEER]]);
  } finally {
    h.restore();
  }
});

test("a hidden DM's message never reaches the all-rooms listeners", async () => {
  seedCommunity();
  const h = await mount(1, async () => ({ id: dmId(0) }));
  try {
    await h.settle();
    const all = [];
    const stop = h.relayClient.liveRooms.listenAllRooms((e) => all.push(e.id));
    await h.deliver(h.chunkOf(dmId(0)), message("evt-1", dmId(0)));
    stop();
    assert.deepEqual(all, []);
  } finally {
    h.restore();
  }
});

test("teardown before the reopen settles keeps the generation fence", async () => {
  seedCommunity();
  const reopened = [];
  const h = await mount(1, async ({ pubkeys }) => {
    reopened.push(pubkeys);
    return { id: dmId(0) };
  });
  try {
    await h.settle();
    const chunk = h.chunkOf(dmId(0));
    await h.act(async () => {
      chunk.onEvent(message("evt-1", dmId(0)));
      // Unmount while the member read is still in flight.
      h.unmount();
    });
    await h.settle();
    assert.equal(reopened.length, 0);
  } finally {
    h.restore();
  }
});

test("a bundle reset (community switch) puts the hidden DMs back in the chunks", async () => {
  seedCommunity();
  const h = await mount(2);
  try {
    await h.settle();
    assert.ok(h.chunkOf(dmId(0)));
    await h.act(async () => {
      h.relayClient.liveRooms.reset();
      await new Promise((r) => setTimeout(r, 5));
    });
    await h.settle();
    const chunk = h.chunkOf(dmId(0));
    assert.ok(chunk, "re-applied after the reset");
    assert.deepEqual(chunk.filter["#h"], [dmId(0), dmId(1)]);
  } finally {
    h.restore();
  }
});

test("the huddle window carries no hidden DMs", async () => {
  seedCommunity();
  globalThis.isTauri = true;
  tauriInternals.metadata = {
    currentWindow: { label: "huddle-0f8fad5b-d9cb-469f-a165-70867728950e" },
  };
  const h = await mount(2);
  try {
    await h.settle();
    assert.equal(h.subscriptions.length, 0);
  } finally {
    h.restore();
  }
});
