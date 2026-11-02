/**
 * Opening a thread from its root id resolves the root in the background
 * (docs/message-sync.md#opening-a-thread-from-its-root-id): store and relay
 * together, retries at 2/5/15 s while connected, asked again on every
 * reconnect, deleted only on proof.
 */
import assert from "node:assert/strict";
import { afterEach, mock, test } from "node:test";

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import React, { act } from "react";
import { createRoot } from "react-dom/client";

import {
  getRememberedRouteEvent,
  rememberRouteEvents,
} from "./lib/routeEventCache.ts";
import {
  setThreadRootDeps,
  THREAD_ROOT_RETRY_DELAYS_MS,
  useThreadRoot,
} from "./useThreadRoot.ts";

const CHANNEL = "room";
const ROOT = "r".repeat(64);
const ALICE = "a".repeat(64);
const MALLORY = "m".repeat(64);

function event(id, kind, pubkey, target = null) {
  const tags = [["h", CHANNEL]];
  if (target) tags.push(["e", target]);
  return { id, kind, pubkey, created_at: 1, content: "", tags, sig: "" };
}
const rootEvent = event(ROOT, 9, ALICE);

let mounted = null;
let restore = null;

afterEach(async () => {
  mock.timers.reset();
  if (mounted) {
    await act(async () => mounted.root.unmount());
    mounted = null;
  }
  restore?.();
  restore = null;
});

/** A fake connection the hook subscribes to. */
function connection(initial) {
  let state = initial;
  const listeners = new Set();
  return {
    deps: {
      connectionState: () => state,
      subscribeToConnectionState: (listener) => {
        listeners.add(listener);
        listener(state);
        return () => listeners.delete(listener);
      },
    },
    set: async (next) => {
      state = next;
      await act(async () => {
        for (const listener of listeners) listener(next);
      });
    },
  };
}

async function flush() {
  await act(async () => {
    for (let i = 0; i < 5; i += 1) await Promise.resolve();
  });
}

async function mount({
  stored = async () => null,
  relay,
  state = "connected",
  loadedEvents = [],
  remembered = [],
}) {
  const link = connection(state);
  const relayCalls = [];
  const storeCalls = [];
  restore = setThreadRootDeps({
    stored: (...args) => {
      storeCalls.push(args);
      return stored(...args);
    },
    relay: async (...args) => {
      relayCalls.push(args);
      return relay(relayCalls.length);
    },
    ...link.deps,
  });
  const queryClient = new QueryClient();
  rememberRouteEvents(queryClient, remembered);
  const result = { current: null };
  const setLoaded = { current: null };
  function Harness() {
    const [loaded, setLoadedEvents] = React.useState(loadedEvents);
    setLoaded.current = setLoadedEvents;
    result.current = useThreadRoot({
      channelId: CHANNEL,
      enabled: true,
      loadedEvents: loaded,
      rootId: ROOT,
    });
    return null;
  }
  const container = document.createElement("div");
  const root = createRoot(container);
  await act(async () =>
    root.render(
      React.createElement(
        QueryClientProvider,
        { client: queryClient },
        React.createElement(Harness),
      ),
    ),
  );
  mounted = { root };
  await flush();
  const load = async (events) => {
    await act(async () => setLoaded.current(events));
    await flush();
  };
  return { link, load, queryClient, relayCalls, result, storeCalls };
}

test("the stored root draws before the relay answers", async () => {
  let answer;
  const pending = new Promise((resolve) => {
    answer = resolve;
  });
  const { result } = await mount({
    stored: async () => ({ event: rootEvent, deleted: false }),
    relay: () => pending,
  });
  assert.equal(result.current.event?.id, ROOT);
  assert.equal(result.current.deleted, false);
  answer([rootEvent]);
  await flush();
});

test("a failed relay request is retried after 2, 5 and 15 s while connected", async () => {
  mock.timers.enable({ apis: ["setTimeout"] });
  const { relayCalls, result } = await mount({ relay: () => null });
  assert.equal(relayCalls.length, 1);
  for (const [index, delay] of THREAD_ROOT_RETRY_DELAYS_MS.entries()) {
    mock.timers.tick(delay - 1);
    await flush();
    assert.equal(relayCalls.length, index + 1, `not before ${delay} ms`);
    mock.timers.tick(1);
    await flush();
    assert.equal(relayCalls.length, index + 2, `after ${delay} ms`);
  }
  mock.timers.tick(60_000);
  await flush();
  assert.equal(relayCalls.length, 4, "no more timed retries");
  assert.equal(result.current.deleted, false, "a failure never means deleted");
});

test("an empty answer is asked again when the connection comes back", async () => {
  const { link, relayCalls, result } = await mount({
    relay: (call) => (call === 1 ? [] : [rootEvent]),
  });
  assert.equal(relayCalls.length, 1);
  assert.equal(result.current.event, null);
  await link.set("reconnecting");
  await link.set("connected");
  await flush();
  assert.equal(relayCalls.length, 2);
  assert.equal(result.current.event?.id, ROOT);
});

test("an offline cold start asks again on the first connect", async () => {
  mock.timers.enable({ apis: ["setTimeout"] });
  const { link, relayCalls, result } = await mount({
    relay: (call) => (call === 1 ? null : [rootEvent]),
    state: "disconnected",
  });
  mock.timers.tick(30_000);
  await flush();
  assert.equal(relayCalls.length, 1, "no timed retries while offline");
  await link.set("connected");
  await flush();
  assert.equal(relayCalls.length, 2);
  assert.equal(result.current.event?.id, ROOT);
});

test("a found root is remembered and never asked again", async () => {
  const { link, queryClient, relayCalls } = await mount({ relay: () => [rootEvent] });
  assert.equal(getRememberedRouteEvent(queryClient, ROOT)?.id, ROOT);
  await link.set("reconnecting");
  await link.set("connected");
  await flush();
  assert.equal(relayCalls.length, 1);
});

test("a removal (9005) marks the root deleted", async () => {
  const { result } = await mount({
    relay: () => [event("d".repeat(64), 9005, MALLORY, ROOT)],
  });
  assert.equal(result.current.deleted, true);
});

test("the author's deletion marks the root deleted", async () => {
  const { result } = await mount({
    relay: () => [rootEvent, event("d".repeat(64), 5, ALICE, ROOT)],
  });
  assert.equal(result.current.deleted, true);
  assert.equal(result.current.event?.id, ROOT);
});

test("another author's deletion leaves the root found", async () => {
  const { result } = await mount({
    relay: () => [rootEvent, event("d".repeat(64), 5, MALLORY, ROOT)],
  });
  assert.equal(result.current.deleted, false);
  assert.equal(result.current.event?.id, ROOT);
});

test("a loaded root asks neither the store nor the relay", async () => {
  const { relayCalls, result, storeCalls } = await mount({
    loadedEvents: [rootEvent],
    relay: () => [rootEvent],
  });
  assert.equal(result.current.event?.id, ROOT);
  assert.equal(relayCalls.length, 0, "no strong-consistency query");
  assert.equal(storeCalls.length, 0);
});

test("a root that leaves the loaded events is asked for", async () => {
  const { load, relayCalls, result } = await mount({
    loadedEvents: [rootEvent],
    relay: () => [rootEvent],
  });
  await load([]);
  assert.equal(relayCalls.length, 1);
  assert.equal(result.current.event?.id, ROOT);
});

test("a lone author's deletion answered before the stored root counts once the store answers", async () => {
  let storedAnswer;
  const stored = new Promise((resolve) => {
    storedAnswer = resolve;
  });
  const { result } = await mount({
    stored: () => stored,
    relay: () => [event("d".repeat(64), 5, ALICE, ROOT)],
  });
  assert.equal(result.current.deleted, false, "author not known yet");
  // The store never saw the deletion (it happened while offline).
  storedAnswer({ event: rootEvent, deleted: false });
  await flush();
  assert.equal(result.current.event?.id, ROOT);
  assert.equal(result.current.deleted, true);
});

test("a lone author's deletion counts once the root loads", async () => {
  const { load, relayCalls, result } = await mount({
    relay: () => [event("d".repeat(64), 5, ALICE, ROOT)],
  });
  assert.equal(relayCalls.length, 1);
  assert.equal(result.current.deleted, false);
  await load([rootEvent]);
  assert.equal(result.current.deleted, true);
});

test("a lone deletion by another author stays unresolved once the author is known", async () => {
  let storedAnswer;
  const stored = new Promise((resolve) => {
    storedAnswer = resolve;
  });
  const { result } = await mount({
    stored: () => stored,
    relay: () => [event("d".repeat(64), 5, MALLORY, ROOT)],
  });
  storedAnswer({ event: rootEvent, deleted: false });
  await flush();
  assert.equal(result.current.event?.id, ROOT);
  assert.equal(result.current.deleted, false);
});

test("a remembered root from another room is not used", async () => {
  const elsewhere = { ...rootEvent, tags: [["h", "other-room"]] };
  const { result } = await mount({ remembered: [elsewhere], relay: () => [] });
  assert.equal(result.current.event, null);
});

test("a stored deletion stays when the relay still returns the root", async () => {
  let answer;
  const pending = new Promise((resolve) => {
    answer = resolve;
  });
  const { result } = await mount({
    stored: async () => ({ event: null, deleted: true }),
    relay: () => pending,
  });
  assert.equal(result.current.deleted, true);
  answer([rootEvent]);
  await flush();
  assert.equal(result.current.deleted, true);
  assert.equal(result.current.event?.id, ROOT);
});

test("a stored copy arriving after the relay answered is ignored", async () => {
  let storedAnswer;
  const stored = new Promise((resolve) => {
    storedAnswer = resolve;
  });
  const { result } = await mount({
    stored: () => stored,
    relay: () => [rootEvent],
  });
  assert.equal(result.current.event?.id, ROOT);
  storedAnswer({ event: null, deleted: true });
  await flush();
  assert.equal(result.current.deleted, false);
});
