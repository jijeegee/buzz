import assert from "node:assert/strict";
import { afterEach, mock, test } from "node:test";

// Smart routing's composer state through the real hook with an injected
// router (no IPC, no model): Enter never waits, the publish queues the sent
// message in its channel's batch, one call routes the batch, each message
// is delivered at most once, and an edit in between fences it out. Typing
// never reaches the hook (the composer's onUpdate does not call it; see
// smartRoutingComposerWiring.test.mjs).

const React = (await import("react")).default;
const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { useAutoAssign } = await import("./useAutoAssign.ts");
const status = await import("../lib/autoRouteStatus.ts");
const batcher = await import("../lib/autoRouteBatcher.ts");
const ledger = await import("../lib/autoRouteLedger.ts");

const CODER = "aa".repeat(32);
const TRANSLATOR = "bb".repeat(32);
const ROSTER = {
  roster: [
    { pubkey: CODER, name: "Coder", description: "Writes code." },
    { pubkey: TRANSLATOR, name: "Translator", description: null },
  ],
  humans: ["Jiho"],
};
const message = (id, tags = []) => ({
  id,
  content: `text ${id}`,
  tags: [["h", "chan"], ...tags],
});

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
async function deliver(sent, pubkeys, isCurrent, route) {
  if (!isCurrent()) return false;
  deliveries.push(
    route ? { id: sent.id, pubkeys, route } : { id: sent.id, pubkeys },
  );
  return true;
}

let mounted = null;

async function mount(overrides = {}) {
  const hook = { current: null };
  const options = {
    addressedAgentCount: 0,
    channelId: "chan",
    channelType: "stream",
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
  mock.timers.enable({ apis: ["setTimeout", "Date"], now: 1_000_000 });
  return hook;
}

afterEach(async () => {
  mock.timers.reset();
  if (mounted) {
    await act(async () => mounted.root.unmount());
    mounted = null;
  }
  calls = [];
  deliveries = [];
  batcher.resetAutoRouteBatches();
  ledger.resetAutoRouteLedger();
  status.resetAutoRouteStatus();
});

async function drain() {
  for (let i = 0; i < 20; i += 1) await Promise.resolve();
}

const send = (hook, sent, text = sent.content) =>
  hook.current.routeAfterSend(text, deliver)(sent);

async function settleCall(index, result) {
  calls[index].resolve(result);
  await drain();
}

const routed = (...groups) => ({
  decision: "routed",
  groups: groups.map(([messageIds, pubkeys, relation = "new", of = null]) => ({
    messageIds,
    pubkeys,
    relation,
    of,
  })),
});

const statusOf = (id) => status.getAutoRouteStatus(id);

test("one call routes the sends of a quiet 3 s and delivers each group", async () => {
  const recent = [
    { pubkey: CODER, isOwner: false, content: "done", createdAt: 1 },
  ];
  let excluded = null;
  const hook = await mount({
    getRecent: (first, exclude) => {
      excluded = [first.id, [...exclude]];
      return recent;
    },
  });
  const onPublished = hook.current.routeAfterSend("fix the build", deliver);
  assert.equal(typeof onPublished, "function");
  onPublished(message("e1"));
  mock.timers.tick(2_000);
  send(hook, message("e2"));
  mock.timers.tick(2_999);
  assert.equal(calls.length, 0, "waits 3 s after the last send");
  assert.equal(statusOf("e1")?.status, "routing");

  mock.timers.tick(1);
  assert.equal(calls.length, 1);
  const { input } = calls[0];
  assert.deepEqual(
    input.messages.map((m) => [m.id, m.text]),
    [
      ["e1", "fix the build"],
      ["e2", "text e2"],
    ],
  );
  assert.equal(input.phase, "send");
  assert.equal(input.channelId, "chan");
  assert.deepEqual(input.recent, recent);
  assert.deepEqual(excluded, ["e1", ["e1", "e2"]], "batch is not RECENT");

  await settleCall(0, routed([["e1"], [CODER]], [["e2"], []]));
  assert.deepEqual(deliveries, [{ id: "e1", pubkeys: [CODER] }]);
  assert.equal(statusOf("e1")?.status, "delivered");
  assert.equal(statusOf("e2"), null, "nobody fit: no trace");
  assert.ok(ledger.wasAutoRouteDelivered("e1"));

  // Exactly once: the same message published again is never re-routed.
  send(hook, message("e1"));
  mock.timers.tick(20_000);
  assert.equal(calls.length, 1);
});

test("a draft in the composer holds the batch, at most 20 s", async () => {
  const hook = await mount();
  send(hook, message("e1"));
  batcher.holdAutoRouteBatch("chan", "composer", true);
  mock.timers.tick(10_000);
  assert.equal(calls.length, 0, "held while the owner is typing");
  batcher.holdAutoRouteBatch("chan", "composer", false);
  mock.timers.tick(0);
  assert.equal(calls.length, 1, "fires once the draft clears");

  send(hook, message("e2"));
  batcher.holdAutoRouteBatch("chan", "composer", true);
  mock.timers.tick(19_999);
  assert.equal(calls.length, 1);
  mock.timers.tick(1);
  assert.equal(calls.length, 2, "never waits past 20 s");
});

test("a follow-up goes to the same agent with a route note", async () => {
  const hook = await mount();
  const reply = [
    ["e", "root1", "", "root"],
    ["e", "root1", "", "reply"],
  ];
  send(hook, message("e1", reply));
  mock.timers.tick(3_000);
  await settleCall(0, routed([["e1"], [CODER]]));

  send(hook, message("e2"));
  mock.timers.tick(3_000);
  assert.deepEqual(calls[1].input.prior, [
    { id: "e1", agents: [CODER], text: "text e1" },
  ]);
  await settleCall(1, routed([["e2"], [CODER], "amend", "e1"]));
  assert.deepEqual(deliveries[1], {
    id: "e2",
    pubkeys: [CODER],
    route: { relation: "amend", of: "e1", threadRoot: "root1" },
  });
});

test("an edit before the pick returns is never delivered to", async () => {
  const hook = await mount();
  send(hook, message("e1"));
  mock.timers.tick(3_000);
  status.cancelAutoRoute("e1");
  await settleCall(0, routed([["e1"], [CODER]]));
  assert.deepEqual(deliveries, []);
  assert.equal(statusOf("e1"), null);
});

test("a failed route shows Not delivered; none and not-configured stay silent", async () => {
  const hook = await mount();
  const results = [
    ["e1", { decision: "skipped", reason: "provider-error" }, "failed"],
    ["e2", routed([["e2"], []]), null],
    ["e3", { decision: "skipped", reason: "not-configured" }, null],
  ];
  for (const [index, [id, result, expected]] of results.entries()) {
    send(hook, message(id));
    mock.timers.tick(3_000);
    await settleCall(index, result);
    assert.equal(statusOf(id)?.status ?? null, expected, id);
  }
  assert.equal(calls.length, 3, "one call per batch, no retry");
  assert.deepEqual(deliveries, []);
});

const SOFT = (pubkey) => ["mention", pubkey, "soft"];
const OUTSIDER = "cc".repeat(32);

test("soft mentions: judged by the router, the fallback on failure, outsiders always", async () => {
  const hook = await mount();
  send(hook, message("e1", [SOFT(TRANSLATOR), SOFT(CODER)]));
  send(hook, message("e2", [SOFT(TRANSLATOR)]));
  send(hook, message("e3", [SOFT(OUTSIDER)]));
  mock.timers.tick(3_000);
  assert.deepEqual(calls[0].input.messages[0].mentioned, [TRANSLATOR, CODER]);
  await settleCall(0, routed([["e1"], [CODER]], [["e2", "e3"], []]));
  assert.deepEqual(deliveries, [
    { id: "e1", pubkeys: [CODER] },
    { id: "e3", pubkeys: [OUTSIDER] },
  ]);

  send(hook, message("e4", [SOFT(TRANSLATOR)]));
  mock.timers.tick(3_000);
  await settleCall(1, { decision: "skipped", reason: "timeout" });
  assert.deepEqual(deliveries[2], { id: "e4", pubkeys: [TRANSLATOR] });
  assert.equal(statusOf("e4")?.status, "delivered");
});

test("a closed gate never calls the router", async () => {
  for (const override of [
    { routerActive: false },
    { routerReady: false },
    { channelType: "dm" },
    { addressedAgentCount: 1 },
    { isEditing: true },
    { getRoster: () => ({ roster: [], humans: [] }) },
  ]) {
    const hook = await mount(override);
    assert.equal(
      hook.current.routeAfterSend("fix the windows build", deliver),
      null,
      JSON.stringify(Object.keys(override)),
    );
    mock.timers.reset();
    await act(async () => mounted.root.unmount());
    mounted = null;
  }
  assert.equal(calls.length, 0);
});
