import assert from "node:assert/strict";
import test from "node:test";

import {
  backoffDelayMs,
  chunkChannels,
  InactiveAccountWatcher,
  MAX_CHANNELS_PER_REQ,
} from "./watcher.ts";
import { createWatchedUnreadStore } from "./store.ts";

const ME = "a".repeat(64);
const OTHER = "b".repeat(64);
const flush = () => new Promise((resolve) => setImmediate(resolve));

function fakeClient(log) {
  const client = {
    lives: new Map(),
    options: null,
    disconnected: false,
    async connect() {
      if (log.failConnect > 0) {
        log.failConnect -= 1;
        throw new Error("down");
      }
    },
    async fetchEvents() {
      return [];
    },
    async subscribeLive(subId, filter, onEvent) {
      client.lives.set(subId, { filter, onEvent });
      log.reqs.push([subId, filter]);
    },
    unsubscribe(subId) {
      client.lives.delete(subId);
      log.closes.push(subId);
    },
    disconnect() {
      client.disconnected = true;
    },
    emit(subId, event) {
      client.lives.get(subId)?.onEvent(event);
    },
  };
  return client;
}

function setup({
  channels = [{ id: "ch", channelType: "stream" }],
  failConnect = 0,
} = {}) {
  const log = {
    reqs: [],
    closes: [],
    notified: [],
    clients: [],
    failConnect,
    counts: 0,
  };
  const timers = [];
  let listed = channels;
  const store = createWatchedUnreadStore();
  const watcher = new InactiveAccountWatcher(`o|${ME}`, "wss://o", ME, {
    createClient: (_url, options) => {
      const client = fakeClient(log);
      client.options = options;
      log.clients.push(client);
      return client;
    },
    authFrame: async () => ({ pendingId: "x", payload: {} }),
    decrypt: async () => "{}",
    readContext: () => ({
      settings: {
        desktopEnabled: true,
        slotAlertsEnabled: { dm: true, mention: true, thread_reply: true },
      },
      participatedRootIds: new Set(),
      followedRootIds: new Set(),
      authoredRootIds: new Set(),
      mutedRootIds: new Set(),
    }),
    notify: (effect, event) => log.notified.push([effect.notify, event.id]),
    store,
    listChannels: async () => listed,
    countUnread: async () => {
      log.counts += 1;
      return { hasUnread: false, mentionCount: 0 };
    },
    now: () => 1_000_000,
    random: () => 1,
    setTimer: (fn, ms) => {
      timers.push({ fn, ms });
      return timers.length - 1;
    },
    clearTimer: (handle) => {
      if (timers[handle]) timers[handle].fn = () => {};
    },
  });
  return {
    watcher,
    log,
    timers,
    store,
    setChannels: (next) => {
      listed = next;
    },
    client: () => log.clients.at(-1),
  };
}

const msg = (id, extra = {}) => ({
  id,
  pubkey: OTHER,
  kind: 9,
  content: "hi",
  created_at: 1_000,
  tags: [["h", "ch"]],
  ...extra,
});

test("backoff grows to a minute and is jittered", () => {
  assert.equal(
    backoffDelayMs(0, () => 1),
    1_000,
  );
  assert.equal(
    backoffDelayMs(3, () => 1),
    8_000,
  );
  assert.equal(
    backoffDelayMs(20, () => 1),
    60_000,
  );
  assert.equal(
    backoffDelayMs(3, () => 0),
    4_000,
  );
});

test("channels are split at the relay's per-REQ cap", () => {
  const ids = Array.from(
    { length: MAX_CHANNELS_PER_REQ * 2 + 1 },
    (_, i) => `c${i}`,
  );
  assert.deepEqual(
    chunkChannels(ids, MAX_CHANNELS_PER_REQ).map((c) => c.length),
    [128, 128, 1],
  );
});

test("start subscribes membership, read state and channel chunks, then counts", async () => {
  const many = Array.from({ length: 130 }, (_, i) => ({
    id: `c${i}`,
    channelType: "stream",
  }));
  const { watcher, log } = setup({ channels: many });
  watcher.start();
  await flush();
  assert.deepEqual(
    log.reqs.map(([id]) => id),
    ["w-mem", "w-rs", "w-ch-0", "w-ch-1"],
  );
  const [, mem] = log.reqs[0];
  assert.deepEqual(mem["#p"], [ME]);
  assert.equal(log.reqs[2][1]["#h"].length, 128);
  assert.equal(log.reqs[3][1]["#h"].length, 2);
  assert.equal(log.reqs[2][1].since, 1_000);
  assert.equal(log.counts, 1);
});

test("a message notifies once, bumps unread, and replays are ignored", async () => {
  const { watcher, log, store, client } = setup({
    channels: [{ id: "ch", channelType: "dm" }],
  });
  watcher.start();
  await flush();
  client().emit("w-ch-0", msg("m1"));
  client().emit("w-ch-0", msg("m1"));
  assert.deepEqual(log.notified, [["dm", "m1"]]);
  assert.deepEqual(store.snapshot().get(`o|${ME}`), {
    hasUnread: true,
    mentionCount: 0,
  });
});

test("a membership event re-lists channels and resubscribes", async () => {
  const { watcher, log, timers, setChannels, client } = setup();
  watcher.start();
  await flush();
  setChannels([
    { id: "ch", channelType: "stream" },
    { id: "new-dm", channelType: "dm" },
  ]);
  client().emit("w-mem", {
    id: "n",
    kind: 44100,
    pubkey: OTHER,
    tags: [],
    content: "",
    created_at: 1_000,
  });
  timers.at(-1).fn();
  await flush();
  const last = log.reqs.at(-1);
  assert.equal(last[0], "w-ch-0");
  assert.deepEqual(last[1]["#h"], ["ch", "new-dm"]);
  client().emit("w-ch-0", msg("d1", { tags: [["h", "new-dm"]] }));
  assert.deepEqual(log.notified.at(-1), ["dm", "d1"]);
});

test("losing every channel closes the old chunks", async () => {
  const { watcher, log, timers, setChannels, client } = setup();
  watcher.start();
  await flush();
  setChannels([]);
  client().emit("w-mem", {
    id: "n",
    kind: 44101,
    pubkey: OTHER,
    tags: [],
    content: "",
    created_at: 1_000,
  });
  timers.at(-1).fn();
  await flush();
  assert.deepEqual(log.closes, ["w-ch-0"]);
});

test("a dropped socket reconnects with backoff and resubscribes from the last event", async () => {
  const { watcher, log, timers, client } = setup({
    channels: [{ id: "ch", channelType: "dm" }],
  });
  watcher.start();
  await flush();
  client().emit("w-ch-0", msg("m1", { created_at: 5_000 }));
  const first = client();
  first.options.onClose();
  assert.equal(first.disconnected, true);
  assert.equal(timers.at(-1).ms, 1_000);
  timers.at(-1).fn();
  await flush();
  assert.notEqual(client(), first);
  const resub = log.reqs.filter(([id]) => id === "w-ch-0").at(-1)[1];
  assert.equal(resub.since, 5_000 - 60);
  // The replayed event does not notify again.
  client().emit("w-ch-0", msg("m1", { created_at: 5_000 }));
  assert.equal(log.notified.length, 1);
});

test("failed connects back off further; resume reconnects at once", async () => {
  const { watcher, log, timers } = setup({ failConnect: 2 });
  watcher.start();
  await flush();
  assert.equal(timers.at(-1).ms, 1_000);
  timers.at(-1).fn();
  await flush();
  assert.equal(timers.at(-1).ms, 2_000);
  watcher.resume();
  await flush();
  assert.equal(log.clients.length, 3);
  assert.ok(log.reqs.some(([id]) => id === "w-ch-0"));
});

test("stop disconnects, clears the unread entry and never reconnects", async () => {
  const { watcher, timers, store, client } = setup({
    channels: [{ id: "ch", channelType: "dm" }],
  });
  watcher.start();
  await flush();
  client().emit("w-ch-0", msg("m1"));
  const c = client();
  watcher.stop();
  assert.equal(c.disconnected, true);
  assert.equal(store.snapshot().has(`o|${ME}`), false);
  c.options.onClose();
  assert.equal(timers.length, 0);
});
