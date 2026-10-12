import assert from "node:assert/strict";
import test from "node:test";

import {
  closeAllWebSockets,
  closeSocketsThenReload,
  closeWebSocket,
} from "./relayWebSocketClose.ts";

test("closeWebSocket invokes authoritative native disconnect", async () => {
  const calls = [];
  await closeWebSocket(42, "community switch", async (cmd, args) => {
    calls.push({ cmd, args });
  });

  assert.deepEqual(calls, [
    { cmd: "plugin:websocket|disconnect", args: { id: 42 } },
  ]);
});

test("closeWebSocket is idempotent when the native socket is gone", async () => {
  await closeWebSocket(7, "connection reset", async () => {
    throw new Error("WebSocket connection not found");
  });
});

test("closeAllWebSockets invokes native process-wide teardown", async () => {
  const calls = [];
  await closeAllWebSockets(async (cmd, args) => calls.push({ cmd, args }));
  assert.deepEqual(calls, [
    { cmd: "plugin:websocket|disconnect_all", args: undefined },
  ]);
});

test("closeSocketsThenReload closes every socket before reloading", async () => {
  const order = [];
  await closeSocketsThenReload({
    closeAll: async () => {
      order.push("close");
    },
    reload: () => order.push("reload"),
  });
  assert.deepEqual(order, ["close", "reload"]);
});

test("closeSocketsThenReload still reloads when the close hangs", async () => {
  let reloaded = false;
  const started = Date.now();
  await closeSocketsThenReload({
    closeAll: () => new Promise(() => {}),
    reload: () => {
      reloaded = true;
    },
    timeoutMs: 20,
  });
  assert.equal(reloaded, true);
  assert.ok(Date.now() - started < 1_000);
});

test("closeSocketsThenReload still reloads when the close fails", async () => {
  let reloaded = false;
  await closeSocketsThenReload({
    closeAll: async () => {
      throw new Error("native layer gone");
    },
    reload: () => {
      reloaded = true;
    },
  });
  assert.equal(reloaded, true);
});
