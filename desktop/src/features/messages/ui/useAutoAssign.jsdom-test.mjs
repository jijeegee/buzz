import assert from "node:assert/strict";
import { afterEach, test } from "node:test";

// Smart routing's composer state through the real hook with an injected
// router (no IPC, no model): one call per send at Enter, the bounded Enter
// wait, and the closed gate. Real timers: the Enter wait is the behavior
// under test. Typing never reaches the hook (the composer's onUpdate does not
// call it; see smartRoutingComposerWiring.test.mjs).

const React = (await import("react")).default;
const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { AUTO_ASSIGN_SEND_WAIT_MS, useAutoAssign } = await import(
  "./useAutoAssign.ts"
);

const CODER = "aa".repeat(32);
const TRANSLATOR = "bb".repeat(32);
const ROSTER = {
  roster: [
    { pubkey: CODER, name: "Coder", description: "Writes code." },
    { pubkey: TRANSLATOR, name: "Translator", description: null },
  ],
  humans: ["Jiho"],
};

let calls = [];
/** Each call gets a deferred the test settles. */
function deferredRoute() {
  return (input) => {
    let resolve;
    const promise = new Promise((r) => {
      resolve = r;
    });
    calls.push({ input, resolve });
    return promise;
  };
}

const sleep = (ms) =>
  act(async () => {
    await new Promise((resolve) => setTimeout(resolve, ms));
  });

let mounted = null;

async function mount(overrides = {}) {
  const hook = { current: null };
  const options = {
    addressedAgentCount: 0,
    channelId: "chan",
    channelType: "stream",
    getExplicitMentionCount: () => 0,
    getRoster: () => ROSTER,
    isEditing: false,
    route: deferredRoute(),
    routerActive: true,
    routerReady: true,
    threadRoot: null,
    ...overrides,
  };
  function Harness() {
    hook.current = useAutoAssign(options);
    return null;
  }
  const container = document.createElement("div");
  const root = createRoot(container);
  await act(async () => root.render(React.createElement(Harness)));
  mounted = { root };
  return hook;
}

afterEach(async () => {
  if (mounted) {
    await act(async () => mounted.root.unmount());
    mounted = null;
  }
  calls = [];
});

const settleCall = (index, result) =>
  act(async () => {
    calls[index].resolve(result);
    await Promise.resolve();
  });

test("the hook has no typing input; each Enter routes exactly once", async () => {
  const hook = await mount();
  assert.deepEqual(Object.keys(hook.current).sort(), [
    "notice",
    "resolveForSend",
  ]);
  assert.equal(calls.length, 0, "mounting never calls the router");

  const sending = hook.current.resolveForSend("fix the windows build");
  await sleep(20);
  assert.equal(calls.length, 1);
  assert.equal(calls[0].input.message, "fix the windows build");
  assert.equal(calls[0].input.phase, "send");
  assert.equal(calls[0].input.channelId, "chan");
  assert.deepEqual(calls[0].input.humans, ["Jiho"]);
  await settleCall(0, { decision: "assigned", pubkeys: [CODER] });
  assert.deepEqual(await sending, [CODER]);

  const again = hook.current.resolveForSend("fix the windows build");
  await sleep(20);
  assert.equal(calls.length, 2, "a second send routes again, no cache");
  await settleCall(1, { decision: "none" });
  assert.deepEqual(await again, []);
});

test("Enter with no answer in time sends unassigned with a quiet notice", async () => {
  const hook = await mount();
  const started = Date.now();
  let sent;
  await act(async () => {
    sent = await hook.current.resolveForSend("fix the windows build");
  });
  const waited = Date.now() - started;
  assert.deepEqual(sent, []);
  assert.equal(calls.length, 1);
  assert.equal(calls[0].input.phase, "send");
  assert.ok(
    waited >= AUTO_ASSIGN_SEND_WAIT_MS - 50 &&
      waited < AUTO_ASSIGN_SEND_WAIT_MS + 800,
    `waited ${waited} ms`,
  );
  assert.equal(
    hook.current.notice,
    "Sent without auto-assign (routing timed out)",
  );
});

test("a slow (subscription) route waits its own longer budget on Enter", async () => {
  const hook = await mount({ sendWaitMs: AUTO_ASSIGN_SEND_WAIT_MS + 1_500 });
  const sending = hook.current.resolveForSend("fix the windows build");
  // Past the default API-key budget, still inside the route's own.
  await sleep(AUTO_ASSIGN_SEND_WAIT_MS + 400);
  calls[0].resolve({ decision: "assigned", pubkeys: [CODER] });
  assert.deepEqual(await sending, [CODER]);
  assert.equal(hook.current.notice, null);
});

test("a failed route sends unassigned; 'not configured' stays silent", async () => {
  const hook = await mount({
    route: () => Promise.reject(new Error("ipc down")),
  });
  let sent;
  await act(async () => {
    sent = await hook.current.resolveForSend("fix the windows build");
  });
  assert.deepEqual(sent, []);
  assert.equal(
    hook.current.notice,
    "Sent without auto-assign (routing failed)",
  );

  const quiet = await (async () => {
    await act(async () => mounted.root.unmount());
    mounted = null;
    return mount({
      route: () =>
        Promise.resolve({ decision: "skipped", reason: "not-configured" }),
    });
  })();
  await act(async () => {
    sent = await quiet.current.resolveForSend("fix the windows build");
  });
  assert.deepEqual(sent, []);
  assert.equal(quiet.current.notice, null);
});

test("a closed gate never calls the router on Enter", async () => {
  for (const override of [
    { routerActive: false },
    { routerReady: false },
    { channelType: "dm" },
    { addressedAgentCount: 1 },
    { isEditing: true },
    { getExplicitMentionCount: () => 1 },
    { getRoster: () => ({ roster: [], humans: [] }) },
  ]) {
    const hook = await mount(override);
    let sent;
    await act(async () => {
      sent = await hook.current.resolveForSend("fix the windows build");
    });
    assert.deepEqual(sent, [], JSON.stringify(Object.keys(override)));
    assert.equal(calls.length, 0, JSON.stringify(Object.keys(override)));
    await act(async () => mounted.root.unmount());
    mounted = null;
  }
});
