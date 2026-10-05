import assert from "node:assert/strict";
import { afterEach, test } from "node:test";

// Smart routing's composer state through the real hook with an injected
// router (no IPC, no model): Enter never waits, the publish routes the sent
// text exactly once, the pick is delivered, and an edit in between fences
// it out. Typing never reaches the hook (the composer's onUpdate does not
// call it; see smartRoutingComposerWiring.test.mjs).

const React = (await import("react")).default;
const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { useAutoAssign } = await import("./useAutoAssign.ts");
const status = await import("../lib/autoRouteStatus.ts");

const CODER = "aa".repeat(32);
const TRANSLATOR = "bb".repeat(32);
const ROSTER = {
  roster: [
    { pubkey: CODER, name: "Coder", description: "Writes code." },
    { pubkey: TRANSLATOR, name: "Translator", description: null },
  ],
  humans: ["Jiho"],
};
const MESSAGE = { id: "m1", content: "fix the windows build", tags: [] };

let calls = [];
let deliveries = [];
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
async function deliver(message, pubkeys, isCurrent) {
  if (!isCurrent()) return false;
  deliveries.push({ id: message.id, pubkeys });
  return true;
}

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
  deliveries = [];
  status.resetAutoRouteStatus();
});

const settleCall = (index, result) =>
  act(async () => {
    calls[index].resolve(result);
    await new Promise((resolve) => setTimeout(resolve, 0));
  });

const statusOf = (id) => status.getAutoRouteStatus(id);

test("Enter never routes; the publish routes once and delivers the pick", async () => {
  const recent = [
    { pubkey: CODER, isOwner: false, content: "done", createdAt: 1 },
  ];
  const hook = await mount({
    getRecent: (message) => (message.id === "m1" ? recent : []),
  });
  assert.deepEqual(Object.keys(hook.current), ["routeAfterSend"]);
  const onPublished = hook.current.routeAfterSend(
    "fix the windows build",
    deliver,
  );
  assert.equal(typeof onPublished, "function");
  assert.equal(calls.length, 0, "the send itself never calls the router");

  onPublished(MESSAGE);
  assert.equal(calls.length, 1);
  assert.equal(calls[0].input.message, "fix the windows build");
  assert.equal(calls[0].input.phase, "send");
  assert.equal(calls[0].input.channelId, "chan");
  assert.deepEqual(calls[0].input.humans, ["Jiho"]);
  assert.deepEqual(calls[0].input.recent, recent, "read at publish time");
  assert.equal(statusOf("m1")?.status, "routing");

  await settleCall(0, { decision: "assigned", pubkeys: [CODER] });
  assert.deepEqual(deliveries, [{ id: "m1", pubkeys: [CODER] }]);
  assert.equal(statusOf("m1")?.status, "delivered");
  assert.deepEqual(statusOf("m1")?.pubkeys, [CODER]);
});

test("an edit before the pick returns is never delivered to", async () => {
  const hook = await mount();
  hook.current.routeAfterSend("fix the windows build", deliver)(MESSAGE);
  status.cancelAutoRoute("m1");
  await settleCall(0, { decision: "assigned", pubkeys: [CODER] });
  assert.deepEqual(deliveries, []);
  assert.equal(statusOf("m1"), null);
});

test("a failed route shows Not delivered; none and not-configured stay silent", async () => {
  const hook = await mount();
  const results = [
    ["m1", { decision: "skipped", reason: "provider-error" }, "failed"],
    ["m2", { decision: "none" }, null],
    ["m3", { decision: "skipped", reason: "not-configured" }, null],
  ];
  for (const [index, [id, result, expected]] of results.entries()) {
    hook.current.routeAfterSend(
      "fix the windows build",
      deliver,
    )({
      ...MESSAGE,
      id,
    });
    await settleCall(index, result);
    assert.equal(statusOf(id)?.status ?? null, expected, id);
  }
  assert.equal(calls.length, 3, "one call per message, no retry");
  assert.deepEqual(deliveries, []);
});

test("a closed gate never calls the router", async () => {
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
    assert.equal(
      hook.current.routeAfterSend("fix the windows build", deliver),
      null,
      JSON.stringify(Object.keys(override)),
    );
    await act(async () => mounted.root.unmount());
    mounted = null;
  }
  assert.equal(calls.length, 0);
});
