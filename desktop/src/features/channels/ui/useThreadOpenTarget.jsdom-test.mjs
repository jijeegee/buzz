import assert from "node:assert/strict";
import test from "node:test";

// A thread opens, as rooms do, at a linked reply, else the first unread reply,
// else where the reader left it, else the newest reply
// (docs/message-sync.md#opening-at-a-position).

const React = (await import("react")).default;
const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { useThreadOpenTarget, threadPositionKey } = await import(
  "./useThreadOpenTarget.ts"
);
const { readingPositionScope, readingPositionStore } = await import(
  "../readingPositionStore.ts"
);

globalThis.IS_REACT_ACT_ENVIRONMENT = true;

const SCOPE = readingPositionScope("a".repeat(64), "wss://relay.example");
const entry = (id) => ({ message: { id } });
const replies = (...ids) => ids.map(entry);

function mount(initial) {
  const results = [];
  let setProps;
  function Probe() {
    const [props, set] = React.useState(initial);
    setProps = set;
    results.push(
      useThreadOpenTarget({
        channelId: "room",
        rootId: "root",
        positionScope: SCOPE,
        opensElsewhere: false,
        replies: [],
        isLoading: false,
        firstUnreadReplyId: null,
        hasOlderReplies: false,
        isFetchingOlder: false,
        fetchOlder: async () => {},
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

test("opens at the first unread reply, near the top", () => {
  window.localStorage.clear();
  const view = mount({
    replies: replies("r1", "r2", "r3"),
    firstUnreadReplyId: "r2",
  });
  try {
    assert.equal(view.last().targetId, "r2");
    assert.equal(view.last().topPx, 8);
    assert.equal(view.last().isResolving, false);
    act(() => view.last().release());
    assert.equal(view.last().targetId, null);
  } finally {
    view.unmount();
  }
});

test("a first unread reply that may be older draws where it is and pages back", async () => {
  window.localStorage.clear();
  let fetched = 0;
  const view = mount({
    replies: replies("r5", "r6"),
    firstUnreadReplyId: "r5",
    hasOlderReplies: true,
    // The page is still out while the test looks.
    fetchOlder: (limitRows) => {
      fetched += 1;
      assert.equal(limitRows, 200);
      return new Promise(() => {});
    },
  });
  try {
    await act(() => Promise.resolve());
    assert.equal(fetched, 1);
    assert.equal(view.last().isResolving, false);
    assert.equal(view.last().targetId, "r5");
    view.update({
      replies: replies("r3", "r4", "r5", "r6"),
      firstUnreadReplyId: "r4",
    });
    assert.equal(view.last().isResolving, false);
    assert.equal(view.last().targetId, "r4");
  } finally {
    view.unmount();
  }
});

test("without unread replies it goes back to where the reader left", () => {
  window.localStorage.clear();
  readingPositionStore.set(SCOPE, threadPositionKey("room", "root"), {
    messageId: "r2",
    topPx: 40,
  });
  const view = mount({ replies: replies("r1", "r2", "r3") });
  try {
    assert.equal(view.last().targetId, "r2");
    assert.equal(view.last().topPx, 40);
    // Reaching the newest reply forgets it.
    view.last().savePosition(null);
    assert.equal(
      readingPositionStore.get(SCOPE, threadPositionKey("room", "root")),
      null,
    );
  } finally {
    view.unmount();
  }
});

test("a link wins, and nothing to open at opens at the newest", () => {
  window.localStorage.clear();
  const linked = mount({
    replies: replies("r1", "r2"),
    firstUnreadReplyId: "r1",
    opensElsewhere: true,
  });
  const plain = mount({ replies: replies("r1", "r2") });
  try {
    assert.equal(linked.last().targetId, null);
    assert.equal(linked.last().isResolving, false);
    assert.equal(plain.last().targetId, null);
    assert.equal(plain.last().isResolving, false);
  } finally {
    linked.unmount();
    plain.unmount();
  }
});

test("nothing is decided while the replies load", () => {
  window.localStorage.clear();
  const view = mount({ isLoading: true, firstUnreadReplyId: null });
  try {
    assert.equal(view.last().isResolving, true);
    view.update({
      isLoading: false,
      replies: replies("r1", "r2"),
      firstUnreadReplyId: "r2",
    });
    assert.equal(view.last().targetId, "r2");
  } finally {
    view.unmount();
  }
});

test("a thread waits for the read state, then trusts no position without it", () => {
  window.localStorage.clear();
  readingPositionStore.set(SCOPE, threadPositionKey("room", "root"), {
    messageId: "r2",
    topPx: 40,
  });
  const view = mount({
    replies: replies("r1", "r2", "r3"),
    readState: "pending",
  });
  try {
    assert.equal(view.last().isResolving, true);
    assert.equal(view.last().targetId, null);
    // Without the read state every reply reads as unread: neither counts.
    view.update({ readState: "missed", firstUnreadReplyId: "r1" });
    assert.equal(view.last().isResolving, false);
    assert.equal(view.last().targetId, null);
    // Being at the newest after the fallback does not forget the spot...
    view.last().savePosition(null);
    assert.deepEqual(
      readingPositionStore.get(SCOPE, threadPositionKey("room", "root")),
      { messageId: "r2", topPx: 40 },
    );
    // ...until the reader has scrolled away once.
    view.last().savePosition({ messageId: "r3", topPx: 5 });
    view.last().savePosition(null);
    assert.equal(
      readingPositionStore.get(SCOPE, threadPositionKey("room", "root")),
      null,
    );
    readingPositionStore.set(SCOPE, threadPositionKey("room", "root"), {
      messageId: "r2",
      topPx: 40,
    });
    // Falling back to the newest keeps the spot for the next visit.
    assert.deepEqual(
      readingPositionStore.get(SCOPE, threadPositionKey("room", "root")),
      { messageId: "r2", topPx: 40 },
    );
  } finally {
    view.unmount();
  }
});

test("opening at a first unread reply forgets the old spot", () => {
  window.localStorage.clear();
  readingPositionStore.set(SCOPE, threadPositionKey("room", "root"), {
    messageId: "r1",
    topPx: 40,
  });
  const view = mount({
    replies: replies("r1", "r2", "r3"),
    firstUnreadReplyId: "r3",
  });
  try {
    assert.equal(view.last().targetId, "r3");
    assert.equal(
      readingPositionStore.get(SCOPE, threadPositionKey("room", "root")),
      null,
    );
  } finally {
    view.unmount();
  }
});

test("older pages wait for the relay: stored replies draw first and stay drawn while it seeks", async () => {
  window.localStorage.clear();
  let found;
  const view = mount({
    replies: replies("r2", "r3"),
    firstUnreadReplyId: "r2",
    repliesConfirmed: false,
    hasOlderReplies: true,
    fetchOlder: () =>
      new Promise((resolve) => {
        found = resolve;
      }),
  });
  try {
    assert.equal(view.last().isResolving, false);
    assert.equal(view.last().targetId, "r2");
    // The list reached the stored target and let go of it.
    act(() => view.last().release());
    await act(async () => {
      view.update({ repliesConfirmed: true });
    });
    assert.equal(view.last().isResolving, false);
    // The seek's target is still taken once it is found.
    await act(async () => {
      view.update({
        replies: replies("r1", "r2", "r3"),
        firstUnreadReplyId: "r1",
        hasOlderReplies: false,
      });
      found(true);
    });
    assert.equal(view.last().isResolving, false);
    assert.equal(view.last().targetId, "r1");
  } finally {
    view.unmount();
  }
});

test("the list's own scroll during a seek is not the reader moving", async () => {
  window.localStorage.clear();
  const view = mount({
    replies: replies("r2", "r3"),
    firstUnreadReplyId: "r2",
    hasOlderReplies: true,
    fetchOlder: () => new Promise(() => {}),
  });
  try {
    await act(() => Promise.resolve());
    const list = document.createElement("div");
    list.innerHTML = '<div data-message-id="r2"></div>';
    document.body.append(list);
    const realNow = Date.now;
    Date.now = () => realNow() + 60_000;
    try {
      act(() => {
        list.dispatchEvent(new window.Event("scroll", { bubbles: true }));
      });
    } finally {
      Date.now = realNow;
      list.remove();
    }
    view.update({
      replies: replies("r1", "r2", "r3"),
      firstUnreadReplyId: "r1",
      hasOlderReplies: false,
    });
    assert.equal(view.last().targetId, "r1");
  } finally {
    view.unmount();
  }
});

test("a reader who scrolled during a seek stays put", async () => {
  window.localStorage.clear();
  const view = mount({
    replies: replies("r2", "r3"),
    firstUnreadReplyId: "r2",
    hasOlderReplies: true,
    fetchOlder: () => new Promise(() => {}),
  });
  try {
    await act(() => Promise.resolve());
    assert.equal(view.last().targetId, "r2");
    act(() => {
      window.dispatchEvent(new window.WheelEvent("wheel", { deltaY: 40 }));
    });
    view.update({
      replies: replies("r1", "r2", "r3"),
      firstUnreadReplyId: "r1",
      hasOlderReplies: false,
    });
    assert.equal(view.last().targetId, "r2");
  } finally {
    view.unmount();
  }
});

test("stored replies decide at once; the relay's replies move the open unless the reader scrolled", () => {
  for (const scrolled of [false, true]) {
    window.localStorage.clear();
    const view = mount({
      replies: replies("r1", "r2"),
      repliesConfirmed: false,
    });
    try {
      assert.equal(view.last().isResolving, false);
      assert.equal(view.last().targetId, null);
      if (scrolled) {
        act(() => {
          window.dispatchEvent(new window.WheelEvent("wheel", { deltaY: 40 }));
        });
      }
      view.update({
        replies: replies("r1", "r2", "r3"),
        firstUnreadReplyId: "r3",
        repliesConfirmed: true,
      });
      assert.equal(view.last().targetId, scrolled ? null : "r3");
    } finally {
      view.unmount();
    }
  }
});
