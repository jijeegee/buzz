import assert from "node:assert/strict";
import test from "node:test";

// A channel opened before the read state's first load waits for it (up to
// READ_STATE_OPEN_WAIT_MS) before taking its open frontier, so unread rows
// are not missed (docs/message-sync.md#opening-at-a-position).

const React = (await import("react")).default;
const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { useChannelUnreadState, READ_STATE_OPEN_WAIT_MS } = await import(
  "./useChannelUnreadState.ts"
);

globalThis.IS_REACT_ACT_ENVIRONMENT = true;

function mount(initial) {
  const results = [];
  let setProps;
  function Probe() {
    const [props, set] = React.useState(initial);
    setProps = set;
    results.push(
      useChannelUnreadState({
        activeChannelId: "room",
        timelineMessages: [],
        currentPubkey: "a".repeat(64),
        openThreadHeadId: null,
        threadReplyTargetId: null,
        expandedThreadReplyIds: new Set(),
        getMessageReadAt: () => null,
        clearChannelUnreadSource: () => {},
        markChannelUnread: () => {},
        markMessageRead: () => {},
        isThreadMuted: () => false,
        readStateVersion: 0,
        ...props,
      }),
    );
    return null;
  }
  const root = createRoot(document.createElement("div"));
  act(() => root.render(React.createElement(Probe)));
  return {
    last: () => results.at(-1),
    update: (patch) => act(() => setProps((props) => ({ ...props, ...patch }))),
    unmount: () => act(() => root.unmount()),
  };
}

test("the open frontier waits for the read state's first load", () => {
  const view = mount({ isReadStateReady: false, getChannelReadAt: () => null });
  try {
    assert.equal(view.last().isOpenFrontierPending, true);
    assert.equal(view.last().openFrontierSeconds, null);
    view.update({ isReadStateReady: true, getChannelReadAt: () => 150 });
    assert.equal(view.last().isOpenFrontierPending, false);
    assert.equal(view.last().openFrontierSeconds, 150);
    // Taken once: a later read marker does not move it.
    view.update({ getChannelReadAt: () => 300, readStateVersion: 1 });
    assert.equal(view.last().openFrontierSeconds, 150);
  } finally {
    view.unmount();
  }
});

test("without the read state in time the frontier is taken as it is", async () => {
  const view = mount({ isReadStateReady: false, getChannelReadAt: () => 120 });
  try {
    assert.equal(view.last().isOpenFrontierPending, true);
    await act(
      () =>
        new Promise((resolve) =>
          setTimeout(resolve, READ_STATE_OPEN_WAIT_MS + 50),
        ),
    );
    assert.equal(view.last().isOpenFrontierPending, false);
    assert.equal(view.last().openFrontierSeconds, 120);
  } finally {
    view.unmount();
  }
});

test("a ready read state takes the frontier at once", () => {
  const view = mount({ isReadStateReady: true, getChannelReadAt: () => 90 });
  try {
    assert.equal(view.last().isOpenFrontierPending, false);
    assert.equal(view.last().openFrontierSeconds, 90);
  } finally {
    view.unmount();
  }
});

test("a read marker from another device keeps the divider", () => {
  const other = "c".repeat(64);
  const row = (id, createdAt) => ({ id, pubkey: other, createdAt });
  const view = mount({
    isReadStateReady: true,
    getChannelReadAt: () => 150,
    timelineMessages: [row("m1", 100), row("m2", 200), row("m3", 300)],
  });
  try {
    assert.equal(view.last().firstUnreadMessageId, "m2");
    view.update({ getChannelReadAt: () => 300, readStateVersion: 1 });
    assert.equal(view.last().openFrontierSeconds, 150);
    assert.equal(view.last().firstUnreadMessageId, "m2");
  } finally {
    view.unmount();
  }
});

test("a read state stuck loading costs one wait, not one per room", async () => {
  const view = mount({ isReadStateReady: false, getChannelReadAt: () => null });
  try {
    await act(
      () =>
        new Promise((resolve) =>
          setTimeout(resolve, READ_STATE_OPEN_WAIT_MS + 50),
        ),
    );
    assert.equal(view.last().isOpenFrontierPending, false);
    view.update({ activeChannelId: "other" });
    assert.equal(view.last().isOpenFrontierPending, false);
    // Another account gets its own wait.
    view.update({ activeChannelId: "third", currentPubkey: "d".repeat(64) });
    assert.equal(view.last().isOpenFrontierPending, true);
  } finally {
    view.unmount();
  }
});
