// Live subscriptions on the read-only client: open past EOSE, replaced by id,
// closed by the relay or by the caller, and a dropped socket reported once.
import assert from "node:assert/strict";
import test from "node:test";

const sends = [];
globalThis.window = {
  setTimeout: (fn, ms) => setTimeout(fn, ms),
  clearTimeout: (id) => clearTimeout(id),
  __TAURI_INTERNALS__: {
    invoke: async (command, args) => {
      if (command === "plugin:websocket|send") {
        sends.push(JSON.parse(args.message.data));
      }
    },
  },
};

const { ReadOnlyRelayClient } = await import("./readOnlyRelayClient.ts");

function connectedClient(options) {
  const client = new ReadOnlyRelayClient("wss://other.example", options);
  client.wsId = 7;
  client.connect = async () => {};
  return client;
}

function deliver(client, frame) {
  return client.handleWsMessage(
    { type: "Text", data: JSON.stringify(frame) },
    client.generation,
  );
}

const ev = (id) => ({
  id,
  kind: 9,
  pubkey: "a",
  content: "",
  tags: [],
  created_at: 1,
});

test("a live subscription keeps delivering after EOSE", async () => {
  sends.length = 0;
  const client = connectedClient();
  const got = [];
  await client.subscribeLive("w-ch-0", { kinds: [9], limit: 5 }, (e) =>
    got.push(e.id),
  );
  assert.deepEqual(sends.at(-1), ["REQ", "w-ch-0", { kinds: [9], limit: 5 }]);
  await deliver(client, ["EVENT", "w-ch-0", ev("stored")]);
  await deliver(client, ["EOSE", "w-ch-0"]);
  await deliver(client, ["EVENT", "w-ch-0", ev("live")]);
  assert.deepEqual(got, ["stored", "live"]);
  // EOSE does not close a live subscription.
  assert.equal(sends.filter((frame) => frame[0] === "CLOSE").length, 0);
});

test("reusing an id replaces the handler; unsubscribe sends CLOSE once", async () => {
  sends.length = 0;
  const client = connectedClient();
  const first = [];
  const second = [];
  await client.subscribeLive("w-ch-0", { kinds: [9], limit: 1 }, (e) =>
    first.push(e.id),
  );
  await client.subscribeLive("w-ch-0", { kinds: [9], limit: 1 }, (e) =>
    second.push(e.id),
  );
  await deliver(client, ["EVENT", "w-ch-0", ev("x")]);
  assert.deepEqual([first, second], [[], ["x"]]);
  client.unsubscribe("w-ch-0");
  client.unsubscribe("w-ch-0");
  assert.deepEqual(
    sends.filter((frame) => frame[0] === "CLOSE"),
    [["CLOSE", "w-ch-0"]],
  );
  await deliver(client, ["EVENT", "w-ch-0", ev("late")]);
  assert.deepEqual(second, ["x"]);
});

test("a relay CLOSED ends the subscription and tells its owner", async () => {
  const client = connectedClient();
  let closed = null;
  await client.subscribeLive(
    "w-mem",
    { kinds: [1], limit: 1 },
    () => {},
    (m) => {
      closed = m;
    },
  );
  await deliver(client, ["CLOSED", "w-mem", "restricted: nope"]);
  assert.equal(closed, "restricted: nope");
  assert.equal(client.lives.has("w-mem"), false);
});

test("a socket Close reports onClose; disconnect() alone does not", async () => {
  let drops = 0;
  const client = connectedClient({ onClose: () => drops++ });
  client.disconnect();
  assert.equal(drops, 0);
  const again = connectedClient({ onClose: () => drops++ });
  await again.handleWsMessage({ type: "Close", data: null }, again.generation);
  assert.equal(drops, 1);
  assert.equal(again.isConnected(), false);
});

test("AUTH uses the given account frame, not the active account's", async () => {
  sends.length = 0;
  const calls = [];
  const client = connectedClient({
    authFrame: async (input) => {
      calls.push(input);
      return { pendingId: "evt", payload: { id: "evt" } };
    },
  });
  client.authRequest = {
    pendingEventId: "",
    resolve() {},
    reject() {},
    timeout: 0,
  };
  await deliver(client, ["AUTH", "challenge-1"]);
  assert.deepEqual(calls, [
    { challenge: "challenge-1", relayUrl: "wss://other.example" },
  ]);
  assert.deepEqual(sends.at(-1), ["AUTH", { id: "evt" }]);
});
