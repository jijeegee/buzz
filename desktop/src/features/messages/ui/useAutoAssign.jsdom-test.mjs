import assert from "node:assert/strict";
import { afterEach, test } from "node:test";

// Smart routing's composer state through the real hook with an injected
// router (no IPC, no model): debounced background preview, the text fence,
// the bounded Enter wait, the per-draft call budget, and the
// closed gate. Real timers: the debounce and the Enter wait are the
// behavior under test.

const React = (await import("react")).default;
const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const {
  AUTO_ASSIGN_DEBOUNCE_MS,
  AUTO_ASSIGN_MAX_PREVIEW_CALLS,
  AUTO_ASSIGN_SEND_WAIT_MS,
  useAutoAssign,
} = await import("./useAutoAssign.ts");

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
    draftKey: "chan",
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

const type = (hook, text) => act(async () => hook.current.onText(text));
const settleCall = (index, result) =>
  act(async () => {
    calls[index].resolve(result);
    await Promise.resolve();
  });
const sendNow = async (hook, text) => {
  let sent;
  await act(async () => {
    sent = await hook.current.resolveForSend(text);
  });
  return sent;
};

test("typing debounces into one background preview call; Enter reuses its pick", async () => {
  const hook = await mount();
  await type(hook, "fix");
  await type(hook, "fix the");
  await type(hook, "fix the windows build");
  await sleep(AUTO_ASSIGN_DEBOUNCE_MS - 300);
  assert.equal(calls.length, 0, "no call before the typing pause");
  await sleep(400);
  assert.equal(calls.length, 1);
  assert.equal(calls[0].input.message, "fix the windows build");
  assert.equal(calls[0].input.phase, "preview");
  assert.equal(calls[0].input.channelId, "chan");
  assert.deepEqual(calls[0].input.humans, ["Jiho"]);

  await settleCall(0, { decision: "assigned", pubkeys: [CODER] });
  assert.deepEqual(Object.keys(hook.current).sort(), [
    "notice",
    "onText",
    "resolveForSend",
  ]);
  assert.deepEqual(await sendNow(hook, "fix the windows build"), [CODER]);
  assert.equal(calls.length, 1, "the cached answer is reused, no new call");
});

test("a late answer for an older text is never used for the current one", async () => {
  const hook = await mount();
  await type(hook, "fix the windows build");
  await sleep(AUTO_ASSIGN_DEBOUNCE_MS + 50);
  await type(hook, "thanks all");
  await sleep(AUTO_ASSIGN_DEBOUNCE_MS + 50);
  assert.equal(calls.length, 2);

  await settleCall(0, { decision: "assigned", pubkeys: [CODER] });
  await settleCall(1, { decision: "none" });
  assert.deepEqual(await sendNow(hook, "thanks all"), []);
});

test("an emptied draft forgets its cached picks", async () => {
  const hook = await mount();
  await type(hook, "translate this please");
  await sleep(AUTO_ASSIGN_DEBOUNCE_MS + 50);
  await settleCall(0, { decision: "assigned", pubkeys: [TRANSLATOR] });
  await type(hook, "");
  await type(hook, "translate this please");
  await sleep(AUTO_ASSIGN_DEBOUNCE_MS + 50);
  assert.equal(calls.length, 2);
});

test("Enter waits for the in-flight preview instead of calling again", async () => {
  const hook = await mount();
  await type(hook, "fix the windows build");
  await sleep(AUTO_ASSIGN_DEBOUNCE_MS + 50);
  assert.equal(calls.length, 1);
  const sending = hook.current.resolveForSend("fix the windows build");
  await sleep(200);
  calls[0].resolve({ decision: "assigned", pubkeys: [CODER] });
  assert.deepEqual(await sending, [CODER]);
  assert.equal(calls.length, 1);
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

test("preview calls are capped per draft; Enter still gets one call", async () => {
  const hook = await mount();
  for (let i = 0; i <= AUTO_ASSIGN_MAX_PREVIEW_CALLS; i++) {
    await type(hook, `message number ${i}`);
    await sleep(AUTO_ASSIGN_DEBOUNCE_MS + 30);
  }
  assert.equal(calls.length, AUTO_ASSIGN_MAX_PREVIEW_CALLS);
  assert.ok(calls.every((call) => call.input.phase === "preview"));

  const sending = hook.current.resolveForSend("the final text");
  await sleep(20);
  assert.equal(calls.length, AUTO_ASSIGN_MAX_PREVIEW_CALLS + 1);
  assert.equal(calls.at(-1).input.phase, "send");
  calls.at(-1).resolve({ decision: "none" });
  await sending;
});

test("a closed gate never calls the router, while typing or on Enter", async () => {
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
    await type(hook, "fix the windows build");
    await sleep(AUTO_ASSIGN_DEBOUNCE_MS + 30);
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
