import assert from "node:assert/strict";
import test from "node:test";

const React = (await import("react")).default;
const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { useComposerQuotes } = await import("./useComposerQuotes.ts");

const QUOTED_ID = "a".repeat(64);
const AUTHOR = "b".repeat(64);
const QUOTE_TAG = ["q", QUOTED_ID, "", AUTHOR];

function message(overrides = {}) {
  return {
    author: "Alice",
    body: "Original",
    createdAt: 1,
    depth: 0,
    id: QUOTED_ID,
    pubkey: AUTHOR,
    tags: [],
    time: "9:00",
    ...overrides,
  };
}

async function mount(initialProps) {
  const latest = {};
  const mainCalls = [];
  const threadCalls = [];
  const behavior = { failNext: false };
  const sendMain = async (...args) => {
    mainCalls.push(args);
    if (behavior.failNext) {
      behavior.failNext = false;
      throw new Error("send failed");
    }
    return { id: "sent" };
  };
  const sendThread = async (...args) => {
    threadCalls.push(args);
    return { id: "sent-thread" };
  };
  function Harness(props) {
    Object.assign(
      latest,
      useComposerQuotes({ ...props, sendMain, sendThread }),
    );
    return null;
  }
  const container = document.createElement("div");
  const root = createRoot(container);
  const render = async (props) => {
    await act(async () => {
      root.render(React.createElement(Harness, props));
    });
  };
  await render(initialProps);
  return {
    behavior,
    latest,
    mainCalls,
    render,
    threadCalls,
    unmount: async () => act(async () => root.unmount()),
  };
}

const BASE = { activeChannelId: "chan-1", enabled: true, threadHeadId: null };

test("a main-timeline quote rides the main send as a q tag, then clears", async () => {
  const h = await mount(BASE);
  assert.equal(h.latest.mainScope.target, null);
  await act(async () => h.latest.mainScope.quote(message()));
  assert.equal(h.latest.mainScope.target?.eventId, QUOTED_ID);
  assert.equal(h.latest.threadScope.target, null);

  const imeta = ["imeta", "url https://blossom/x.png"];
  await act(async () => {
    await h.latest.sendMain("hello", ["p1"], [imeta], null, null, false);
  });
  // Placement args pass through untouched: no parent, no thread context.
  assert.deepEqual(h.mainCalls[0], [
    "hello",
    ["p1"],
    [imeta, QUOTE_TAG],
    null,
    null,
    false,
  ]);
  assert.equal(h.latest.mainScope.target, null);

  await act(async () => {
    await h.latest.sendMain("plain", [], undefined);
  });
  assert.equal(h.mainCalls[1][2], undefined, "no quote, no extra tags");
  await h.unmount();
});

test("a failed send keeps the quote for a retry", async () => {
  const h = await mount(BASE);
  await act(async () => h.latest.mainScope.quote(message()));
  h.behavior.failNext = true;
  await act(async () => {
    await assert.rejects(h.latest.sendMain("x", []), /send failed/);
  });
  assert.equal(h.latest.mainScope.target?.eventId, QUOTED_ID);
  await act(async () => {
    await h.latest.sendMain("retry", []);
  });
  assert.deepEqual(h.mainCalls[1][2], [QUOTE_TAG], "the retry still quotes");
  assert.equal(h.latest.mainScope.target, null);
  await h.unmount();
});

test("a thread quote goes to the thread send with its thread context", async () => {
  const h = await mount({ ...BASE, threadHeadId: "c".repeat(64) });
  await act(async () => h.latest.threadScope.quote(message()));
  assert.equal(h.latest.mainScope.target, null);
  const context = {
    parentEventId: "c".repeat(64),
    threadHeadId: "c".repeat(64),
  };
  await act(async () => {
    await h.latest.sendThread("in thread", [], undefined, null, context);
  });
  assert.deepEqual(h.threadCalls[0], [
    "in thread",
    [],
    [QUOTE_TAG],
    null,
    context,
  ]);
  assert.equal(h.mainCalls.length, 0);
  assert.equal(h.latest.threadScope.target, null);
  await h.unmount();
});

test("switching channel or thread drops the pending quote", async () => {
  const h = await mount({ ...BASE, threadHeadId: "c".repeat(64) });
  await act(async () => {
    h.latest.mainScope.quote(message());
    h.latest.threadScope.quote(message());
  });
  await h.render({ ...BASE, threadHeadId: "d".repeat(64) });
  assert.equal(h.latest.threadScope.target, null);
  assert.equal(h.latest.mainScope.target?.eventId, QUOTED_ID);
  await h.render({ ...BASE, activeChannelId: "chan-2" });
  assert.equal(h.latest.mainScope.target, null);
  await h.unmount();
});

test("a disabled scope offers no Quote action and unquotable rows are ignored", async () => {
  const h = await mount({ ...BASE, enabled: false });
  assert.equal(h.latest.mainScope.quote, null);
  await h.render(BASE);
  await act(async () => h.latest.mainScope.quote(message({ pending: true })));
  assert.equal(h.latest.mainScope.target, null);
  await h.unmount();
});
