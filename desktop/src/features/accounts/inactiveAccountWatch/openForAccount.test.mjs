import assert from "node:assert/strict";
import test from "node:test";

import {
  openNotificationForAccount,
  resumeNotificationForAccount,
} from "./openForAccount.ts";

const A = "https://a.example";
const B = "https://b.example";
const ME = "a".repeat(64);
const OTHER = "b".repeat(64);

function memoryStorage() {
  const map = new Map();
  return {
    getItem: (key) => (map.has(key) ? map.get(key) : null),
    setItem: (key, value) => map.set(key, String(value)),
    removeItem: (key) => map.delete(key),
    size: () => map.size,
  };
}

/** One app start: who is open, and what the routing did. */
function start(open, { storage, signedIn = true, now = 1_000, fail } = {}) {
  const log = [];
  const deps = {
    open,
    communities: [
      { id: "ca", relayUrl: "wss://a.example" },
      { id: "cb", relayUrl: "wss://b.example" },
    ],
    isSignedIn: async () => signedIn,
    switchCommunity: (id) => log.push(["community", id]),
    switchAccount: async (pubkey) => {
      if (fail) throw new Error("nope");
      log.push(["account", pubkey]);
    },
    openHere: async (target) => log.push(["open", target]),
    unavailable: (reason) => log.push(["unavailable", reason]),
    storage,
    now: () => now,
  };
  return { deps, log };
}

const target = (account) => ({
  channelId: "ch",
  eventId: "ev",
  kind: 9,
  threadRootId: null,
  ...(account ? { account } : {}),
});

test("a target for the open account opens at once", async () => {
  const storage = memoryStorage();
  const { deps, log } = start({ origin: A, pubkey: ME }, { storage });
  await openNotificationForAccount(target({ origin: A, pubkey: ME }), deps);
  await openNotificationForAccount(target(), deps);
  assert.deepEqual(log, [
    ["open", target()],
    ["open", target()],
  ]);
  assert.equal(storage.size(), 0);
});

test("same server: switch account, then the next start opens the message", async () => {
  const storage = memoryStorage();
  const first = start({ origin: A, pubkey: ME }, { storage });
  await openNotificationForAccount(
    target({ origin: A, pubkey: OTHER }),
    first.deps,
  );
  assert.deepEqual(first.log, [["account", OTHER]]);
  assert.equal(storage.size(), 1);

  const second = start({ origin: A, pubkey: OTHER }, { storage });
  await resumeNotificationForAccount(second.deps);
  assert.deepEqual(second.log, [["open", target()]]);
  assert.equal(storage.size(), 0);
});

test("other server: community first, then account, then the message", async () => {
  const storage = memoryStorage();
  const first = start({ origin: A, pubkey: ME }, { storage });
  await openNotificationForAccount(
    target({ origin: B, pubkey: OTHER }),
    first.deps,
  );
  assert.deepEqual(first.log, [["community", "cb"]]);

  const second = start({ origin: B, pubkey: ME }, { storage });
  await resumeNotificationForAccount(second.deps);
  assert.deepEqual(second.log, [["account", OTHER]]);

  const third = start({ origin: B, pubkey: OTHER }, { storage });
  await resumeNotificationForAccount(third.deps);
  assert.deepEqual(third.log, [["open", target()]]);
  assert.equal(storage.size(), 0);
});

test("a signed-out account is not switched to", async () => {
  const storage = memoryStorage();
  const { deps, log } = start(
    { origin: A, pubkey: ME },
    { storage, signedIn: false },
  );
  await openNotificationForAccount(target({ origin: A, pubkey: OTHER }), deps);
  assert.deepEqual(log, [["unavailable", "signed_out"]]);
  assert.equal(storage.size(), 0);
});

test("a server with no community here is reported, not switched", async () => {
  const storage = memoryStorage();
  const { deps, log } = start({ origin: A, pubkey: ME }, { storage });
  await openNotificationForAccount(
    target({ origin: "https://gone.example", pubkey: OTHER }),
    deps,
  );
  assert.deepEqual(log, [["unavailable", "no_community"]]);
  assert.equal(storage.size(), 0);
});

test("a failed switch clears the pending click", async () => {
  const storage = memoryStorage();
  const { deps, log } = start(
    { origin: A, pubkey: ME },
    { storage, fail: true },
  );
  await openNotificationForAccount(target({ origin: A, pubkey: OTHER }), deps);
  assert.deepEqual(log, [["unavailable", "failed"]]);
  assert.equal(storage.size(), 0);
});

test("an old pending click is dropped at start", async () => {
  const storage = memoryStorage();
  const first = start({ origin: A, pubkey: ME }, { storage, now: 0 });
  await openNotificationForAccount(
    target({ origin: A, pubkey: OTHER }),
    first.deps,
  );
  const later = start({ origin: A, pubkey: ME }, { storage, now: 3 * 60_000 });
  await resumeNotificationForAccount(later.deps);
  assert.deepEqual(later.log, []);
  assert.equal(storage.size(), 0);
});

test("a switch that never lands stops after a few starts", async () => {
  const storage = memoryStorage();
  const first = start({ origin: A, pubkey: ME }, { storage });
  await openNotificationForAccount(
    target({ origin: A, pubkey: OTHER }),
    first.deps,
  );
  const reasons = [];
  for (let i = 0; i < 4; i += 1) {
    const again = start({ origin: A, pubkey: ME }, { storage });
    await resumeNotificationForAccount(again.deps);
    reasons.push(...again.log.map(([kind]) => kind));
  }
  assert.deepEqual(reasons, ["account", "account", "unavailable"]);
  assert.equal(storage.size(), 0);
});

test("nothing pending: start does nothing", async () => {
  const { deps, log } = start(
    { origin: A, pubkey: ME },
    { storage: memoryStorage() },
  );
  await resumeNotificationForAccount(deps);
  assert.deepEqual(log, []);
});

test("built-in server: an account on another server only focuses the app", async () => {
  const storage = memoryStorage();
  const { deps, log } = start({ origin: A, pubkey: ME }, { storage });
  deps.allowsServer = (origin) => origin === A;
  deps.focusApp = () => log.push(["focus"]);
  await openNotificationForAccount(target({ origin: B, pubkey: OTHER }), deps);
  assert.deepEqual(log, [["focus"]]);
  assert.equal(storage.size(), 0);

  // The built-in server itself still switches accounts as before.
  await openNotificationForAccount(target({ origin: A, pubkey: OTHER }), deps);
  assert.deepEqual(log, [["focus"], ["account", OTHER]]);
});
