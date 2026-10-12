// close() is final: work still in flight on a dropped client must not open
// (and authenticate) a new socket behind its owner's back.
import assert from "node:assert/strict";
import test from "node:test";

let connects = 0;
const closed = [];
let releaseConnect = null;
globalThis.window = {
  setTimeout: (fn, ms) => setTimeout(fn, ms),
  clearTimeout: (id) => clearTimeout(id),
  __TAURI_INTERNALS__: {
    transformCallback: () => 1,
    invoke: async (command, args) => {
      if (command === "plugin:websocket|connect") {
        connects += 1;
        if (releaseConnect) await new Promise((r) => (releaseConnect = r));
        return 40 + connects;
      }
      if (command === "plugin:websocket|send") {
        const frame = JSON.parse(args.message.data);
        if (frame[0] === "CLOSE") closed.push(frame[1]);
      }
      if (command === "plugin:websocket|close") closed.push(args?.id);
    },
  },
};

const { ReadOnlyRelayClient } = await import("./readOnlyRelayClient.ts");

test("after close(), late fetch / subscribe / connect never open a socket", async () => {
  connects = 0;
  const client = new ReadOnlyRelayClient("wss://other.example");
  client.close();
  await assert.rejects(client.fetchEvents({ kinds: [9] }), /closed/);
  await assert.rejects(
    client.subscribeLive("w-ch-0", { kinds: [9] }, () => {}),
    /closed/,
  );
  await assert.rejects(client.connect(), /closed/);
  assert.equal(connects, 0);
  assert.equal(client.isConnected(), false);
});

test("a disconnect while the socket is opening does not keep that socket", async () => {
  connects = 0;
  releaseConnect = () => {};
  const client = new ReadOnlyRelayClient("wss://other.example");
  const pending = client.connect();
  await new Promise((r) => setImmediate(r));
  client.close();
  const release = releaseConnect;
  releaseConnect = null;
  release();
  await assert.rejects(pending, /disconnected/);
  assert.equal(connects, 1);
  assert.equal(client.isConnected(), false);
});
