import assert from "node:assert/strict";
import { after, afterEach, before, mock, test } from "node:test";

import {
  handleRemoteCommand,
  REMOTE_START_SETTLE_MS,
} from "./remoteStartHandler.ts";

// The settle window: a `!start` followed by a replayed `!shutdown` within
// REMOTE_START_SETTLE_MS (a reconnect, a newly seen DM) does not start.

const OWNER = "a".repeat(64);
const AGENT = "c".repeat(64);
const started = [];

before(() => {
  globalThis.window = globalThis;
  globalThis.__TAURI_INTERNALS__ = {
    invoke: async (cmd, args) => {
      if (cmd === "list_all_managed_agents") {
        return [
          {
            pubkey: AGENT,
            name: "Agent",
            owner_pubkey: OWNER,
            status: "stopped",
            backend: { type: "local" },
          },
        ];
      }
      if (cmd === "start_managed_agent") {
        started.push(args.pubkey);
        return {
          pubkey: AGENT,
          name: "Agent",
          status: "running",
          backend: { type: "local" },
        };
      }
      throw new Error(`unmocked Tauri command: ${cmd}`);
    },
    transformCallback: () => 1,
  };
});

after(() => {
  delete globalThis.__TAURI_INTERNALS__;
  delete globalThis.window;
});

afterEach(() => {
  mock.timers.reset();
  started.length = 0;
});

const scope = {
  ownerPubkey: OWNER,
  relayUrl: "wss://relay.example",
  openAccount: true,
};

function command(id, content, at) {
  return {
    id,
    kind: 9,
    pubkey: OWNER,
    content,
    created_at: at,
    tags: [
      ["h", "dm-agent"],
      ["p", AGENT],
    ],
    sig: "",
  };
}

const settle = async () => {
  mock.timers.tick(REMOTE_START_SETTLE_MS);
  for (let i = 0; i < 50; i++) await new Promise((r) => setImmediate(r));
};

test("a !start then a replayed !shutdown within the settle window does not start", async () => {
  mock.timers.enable({ apis: ["setTimeout"] });
  const now = Math.floor(Date.now() / 1_000);
  handleRemoteCommand(command("start-a", "!start", now - 2), scope);
  handleRemoteCommand(command("stop-a", "!shutdown", now - 1), scope);
  await settle();
  assert.deepEqual(started, []);
});

test("a !start alone starts once, however often it is delivered", async () => {
  mock.timers.enable({ apis: ["setTimeout"] });
  const now = Math.floor(Date.now() / 1_000);
  const start = command("start-b", "!start", now);
  handleRemoteCommand(start, scope);
  handleRemoteCommand(start, scope);
  await settle();
  assert.deepEqual(started, [AGENT]);
});
