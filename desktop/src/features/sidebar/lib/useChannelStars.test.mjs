import assert from "node:assert/strict";
import { after, before, test } from "node:test";

import { JSDOM } from "jsdom";

const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://localhost",
});

before(() => {
  Object.assign(globalThis, {
    document: dom.window.document,
    HTMLElement: dom.window.HTMLElement,
    IS_REACT_ACT_ENVIRONMENT: true,
    window: dom.window,
  });
});

after(() => dom.window.close());

test("pins persist through remount and unpin; scope switches fence old remote reads and committed UI", async () => {
  const { act, renderHook } = await import("@testing-library/react");
  const { useLayoutEffect } = await import("react");
  const { relayClient } = await import("@/shared/api/relayClient");
  const { ChannelStarSyncManager } = await import("./channelStarsSync.ts");
  const { readChannelStarsStore, storageKey } = await import(
    "./channelStarsStorage.ts"
  );
  const { useChannelStars } = await import("./useChannelStars.ts");
  const proto = ChannelStarSyncManager.prototype;
  const originals = {
    bootstrap: proto.bootstrap,
    publishStars: proto.publishStars,
    subscribeToStars: proto.subscribeToStars,
    reconnect: relayClient.subscribeToReconnects,
  };
  const pending = [];
  const published = [];
  proto.bootstrap = () => new Promise((resolve) => pending.push(resolve));
  proto.publishStars = (store) => published.push(store);
  proto.subscribeToStars = async () => async () => {};
  relayClient.subscribeToReconnects = () => () => {};
  const a = { pubkey: "scope-owner-a", relay: "wss://one.example" };
  const b = { pubkey: "scope-owner-b", relay: a.relay };
  const otherCommunity = { ...a, relay: "wss://two.example" };
  const frames = [];
  const mount = () =>
    renderHook(
      ({ pubkey, relay }) => {
        const pins = useChannelStars(pubkey, relay);
        useLayoutEffect(() => {
          frames.push([pubkey, relay, [...pins.starredChannelIds]]);
        });
        return pins;
      },
      { initialProps: a },
    );
  let hook;
  try {
    hook = mount();
    act(() => hook.result.current.starChannel("same-id"));
    assert.deepEqual([...hook.result.current.starredChannelIds], ["same-id"]);
    assert.equal(
      readChannelStarsStore(a.pubkey, a.relay).channels["same-id"].starred,
      true,
    );
    hook.unmount();
    hook = mount();
    assert.deepEqual([...hook.result.current.starredChannelIds], ["same-id"]);
    const previousOwnerPin = hook.result.current.starChannel;
    hook.rerender(b);
    act(() => previousOwnerPin("stale-menu-action"));
    assert.deepEqual([...hook.result.current.starredChannelIds], []);
    assert.equal(
      readChannelStarsStore(a.pubkey, a.relay).channels["stale-menu-action"],
      undefined,
    );
    hook.rerender(otherCommunity);
    assert.deepEqual([...hook.result.current.starredChannelIds], []);
    assert.ok(
      frames
        .filter(([owner, relay]) => owner !== a.pubkey || relay !== a.relay)
        .every(([, , pins]) => pins.length === 0),
    );
    await act(async () => {
      for (const resolve of pending.splice(0, 3))
        resolve({
          action: "apply-remote",
          data: {
            store: {
              version: 1,
              channels: { late: { starred: true, updatedAt: 100 } },
            },
            createdAt: 100,
            eventId: "old-scope",
          },
        });
    });
    assert.deepEqual([...hook.result.current.starredChannelIds], []);
    assert.equal(
      window.localStorage.getItem(storageKey(a.pubkey, otherCommunity.relay)),
      null,
    );
    hook.rerender(a);
    act(() => hook.result.current.unstarChannel("same-id"));
    assert.deepEqual([...hook.result.current.starredChannelIds], []);
    hook.unmount();
    hook = mount();
    assert.deepEqual([...hook.result.current.starredChannelIds], []);
    assert.equal(
      readChannelStarsStore(a.pubkey, a.relay).channels["same-id"].starred,
      false,
    );
    assert.equal(published.length, 2);
  } finally {
    hook?.unmount();
    Object.assign(proto, {
      bootstrap: originals.bootstrap,
      publishStars: originals.publishStars,
      subscribeToStars: originals.subscribeToStars,
    });
    relayClient.subscribeToReconnects = originals.reconnect;
  }
});

test("legacy pins migrate once to a normalized relay without deleting the original data", async () => {
  const { readChannelStarsStore, storageKey } = await import(
    "./channelStarsStorage.ts"
  );
  const legacy = {
    version: 1,
    channels: { old: { starred: true, updatedAt: 10 } },
  };
  window.localStorage.setItem(
    storageKey("legacy-owner"),
    JSON.stringify(legacy),
  );
  assert.deepEqual(
    readChannelStarsStore("legacy-owner", "wss://ONE.example/"),
    legacy,
  );
  assert.deepEqual(
    readChannelStarsStore("legacy-owner", "wss://one.example"),
    legacy,
  );
  assert.deepEqual(
    readChannelStarsStore("legacy-owner", "wss://two.example").channels,
    {},
  );
  assert.deepEqual(
    JSON.parse(window.localStorage.getItem(storageKey("legacy-owner")))
      .channels,
    legacy.channels,
  );
});

test("same-second star and unstar mutations survive at capacity", async () => {
  const { act, cleanup, renderHook } = await import("@testing-library/react");
  const { relayClient } = await import("@/shared/api/relayClient");
  const { MAX_CHANNEL_STAR_ENTRIES, readChannelStarsStore, storageKey } =
    await import("./channelStarsStorage.ts");
  const { useChannelStars } = await import("./useChannelStars.ts");

  const originalFetchEvents = relayClient.fetchEvents;
  const originalSubscribeLive = relayClient.subscribeLive;
  const originalSubscribeToReconnects = relayClient.subscribeToReconnects;
  const originalDateNow = Date.now;
  const updatedAt = 1_234_567;
  Date.now = () => updatedAt * 1_000;
  relayClient.fetchEvents = async () => [];
  relayClient.subscribeLive = async () => async () => {};
  relayClient.subscribeToReconnects = () => () => {};

  const relayUrl = "wss://relay.example";
  const channels = Object.fromEntries(
    Array.from({ length: MAX_CHANNEL_STAR_ENTRIES }, (_, index) => [
      `z-channel-${String(index).padStart(3, "0")}`,
      { starred: true, updatedAt },
    ]),
  );

  try {
    for (const [pubkey, action, expectedStarred] of [
      ["pk-star", "starChannel", true],
      ["pk-unstar", "unstarChannel", false],
    ]) {
      window.localStorage.setItem(
        storageKey(pubkey),
        JSON.stringify({ version: 1, channels }),
      );
      const { result, unmount } = renderHook(() =>
        useChannelStars(pubkey, relayUrl),
      );

      act(() => result.current[action]("a-target"));

      const persisted = readChannelStarsStore(pubkey, relayUrl);
      assert.equal(
        Object.keys(persisted.channels).length,
        MAX_CHANNEL_STAR_ENTRIES,
      );
      assert.deepEqual(persisted.channels["a-target"], {
        starred: expectedStarred,
        updatedAt,
      });
      unmount();
    }
  } finally {
    cleanup();
    Date.now = originalDateNow;
    relayClient.fetchEvents = originalFetchEvents;
    relayClient.subscribeLive = originalSubscribeLive;
    relayClient.subscribeToReconnects = originalSubscribeToReconnects;
  }
});
