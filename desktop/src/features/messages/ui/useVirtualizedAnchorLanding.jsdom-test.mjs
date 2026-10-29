import assert from "node:assert/strict";
import test from "node:test";

// The list opens hidden and is shown only once its position holds still:
// at the bottom, at a remembered row, or centred on a linked message.

const React = (await import("react")).default;
const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { useVirtualizedAnchorLanding } = await import(
  "./useVirtualizedAnchorLanding.ts"
);

globalThis.IS_REACT_ACT_ENVIRONMENT = true;
globalThis.requestAnimationFrame ??= (callback) =>
  setTimeout(() => callback(performance.now()), 16);
globalThis.cancelAnimationFrame ??= (id) => clearTimeout(id);

function fakeList() {
  const calls = [];
  const list = {
    scrollSize: 1000,
    viewportSize: 400,
    scrollOffset: 0,
    getItemOffset: (index) => index * 100,
    scrollToIndex: (index, options) => calls.push(["index", index, options]),
    scrollTo: (offset) => calls.push(["to", offset]),
  };
  return { list, calls };
}

function mount(list) {
  const host = document.createElement("div");
  host.appendChild(document.createElement("div"));
  const atBottom = [];
  let api;
  function Probe() {
    const hostRef = React.useRef(host);
    const listRef = React.useRef(list);
    const reportRef = React.useRef((value) => atBottom.push(value));
    api = useVirtualizedAnchorLanding(hostRef, listRef, reportRef);
    return null;
  }
  const root = createRoot(document.createElement("div"));
  act(() => root.render(React.createElement(Probe)));
  return {
    host,
    atBottom,
    api: () => api,
    unmount: () => act(() => root.unmount()),
  };
}

const frames = (count) =>
  act(async () => {
    for (let index = 0; index < count; index += 1) {
      await new Promise((resolve) => requestAnimationFrame(resolve));
    }
  });

test("the bottom is shown only after the floor stops moving", async () => {
  const { list } = fakeList();
  const view = mount(list);
  let settled = 0;
  try {
    view.api().landAtBottom(() => {
      settled += 1;
      list.scrollOffset = list.scrollSize - list.viewportSize;
    });
    assert.equal(settled, 1);
    assert.equal(view.host.style.visibility, "hidden");
    // Rows are measured: the floor grows and the settle follows it.
    list.scrollSize = 1400;
    list.scrollOffset = 1000;
    await frames(2);
    assert.equal(view.host.style.visibility, "hidden");
    await frames(3);
    assert.equal(view.host.style.visibility, "");
    assert.deepEqual(view.atBottom, [true]);
  } finally {
    view.unmount();
  }
});

test("a linked message is landed centred and reported while landing", async () => {
  const { list, calls } = fakeList();
  const view = mount(list);
  try {
    assert.equal(
      view.api().land(() => 3, null, "center"),
      true,
    );
    assert.deepEqual(calls[0], ["index", 3, { align: "center" }]);
    assert.equal(view.api().isLandingAt(3), true);
    assert.equal(view.api().isLandingAt(2), false);
    await frames(5);
    assert.equal(view.host.style.visibility, "");
    assert.equal(view.api().isLandingAt(3), false);
  } finally {
    view.unmount();
  }
});

test("unmounting mid-landing stops the frame loop", async () => {
  const { list, calls } = fakeList();
  const view = mount(list);
  view.api().land(() => 3, null, "center");
  view.unmount();
  const before = calls.length;
  await frames(3);
  assert.equal(calls.length, before);
});

test("rows loaded above while hidden do not move the landing", async () => {
  const { list, calls } = fakeList();
  const view = mount(list);
  let index = 3;
  try {
    view.api().land(() => index, null, "center");
    // An older page is put in above the row: it is now row 53.
    index = 53;
    await frames(1);
    assert.deepEqual(calls.at(-1), ["index", 53, { align: "center" }]);
    assert.equal(view.api().isLandingAt(53), true);
    assert.equal(view.api().isLandingAt(3), false);
  } finally {
    view.unmount();
  }
});
