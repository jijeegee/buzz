import assert from "node:assert/strict";
import test from "node:test";

// Where a channel visit opens is decided only on a current window: never on
// a head cache from an earlier run or a window cached before the visit that
// may miss newer unread rows (docs/message-sync.md#opening-at-a-position).

const React = (await import("react")).default;
const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { useChannelOpenAnchor, OPEN_WINDOW_WAIT_MS } = await import(
  "./useChannelOpenAnchor.ts"
);

const { readingPositionScope, readingPositionStore } = await import(
  "../readingPositionStore.ts"
);

globalThis.IS_REACT_ACT_ENVIRONMENT = true;

const message = (id, createdAt) => ({
  id,
  pubkey: "a".repeat(64),
  created_at: createdAt,
  kind: 9,
  tags: [["h", "room"]],
  content: "",
  sig: "",
});
const iso = (seconds) => new Date(seconds * 1000).toISOString();

function mount(initial) {
  const results = [];
  let setProps;
  function Probe() {
    const [props, set] = React.useState(initial);
    setProps = set;
    results.push(
      useChannelOpenAnchor({
        channelId: "room",
        positionScope: null,
        hasOlderMessages: false,
        isFetchingOlder: false,
        fetchOlder: async () => {},
        opensElsewhere: false,
        ...props,
      }),
    );
    return null;
  }
  const host = document.createElement("div");
  const root = createRoot(host);
  act(() => root.render(React.createElement(Probe)));
  return {
    last: () => results.at(-1),
    update: (patch) => act(() => setProps((props) => ({ ...props, ...patch }))),
    unmount: () => act(() => root.unmount()),
  };
}

const rows = [message("old", 100), message("unread", 200)];

test("a head cache from an earlier run waits for the relay's window", () => {
  const view = mount({
    messages: [message("old", 100)],
    windowCommit: null,
    lastMessageAt: iso(200),
    frontierSeconds: 150,
    firstUnreadMessageId: null,
  });
  try {
    assert.equal(view.last().isResolving, true);
    assert.equal(view.last().anchorMessageId, null);
    view.update({
      messages: rows,
      windowCommit: { at: Date.now(), newestRowAt: 200 },
      firstUnreadMessageId: "unread",
    });
    assert.equal(view.last().isResolving, false);
    assert.equal(view.last().anchorMessageId, "unread");
  } finally {
    view.unmount();
  }
});

test("nothing unread and no remembered row opens a head cache at once", () => {
  const view = mount({
    messages: [message("old", 100)],
    windowCommit: null,
    lastMessageAt: iso(100),
    frontierSeconds: 150,
    firstUnreadMessageId: null,
  });
  try {
    assert.equal(view.last().isResolving, false);
    assert.equal(view.last().anchorMessageId, null);
  } finally {
    view.unmount();
  }
});

test("a window cached before the visit is current when it has the last message", () => {
  const view = mount({
    messages: rows,
    windowCommit: { at: Date.now() - 60_000, newestRowAt: 200 },
    lastMessageAt: iso(200),
    frontierSeconds: 150,
    firstUnreadMessageId: "unread",
  });
  try {
    assert.equal(view.last().isResolving, false);
    assert.equal(view.last().anchorMessageId, "unread");
  } finally {
    view.unmount();
  }
});

test("without a current window the visit decides on its rows after the wait", async () => {
  const view = mount({
    messages: rows,
    windowCommit: { at: Date.now() - 60_000, newestRowAt: 200 },
    lastMessageAt: iso(300),
    frontierSeconds: 150,
    firstUnreadMessageId: "unread",
  });
  try {
    assert.equal(view.last().isResolving, true);
    await act(
      () =>
        new Promise((resolve) => setTimeout(resolve, OPEN_WINDOW_WAIT_MS + 50)),
    );
    assert.equal(view.last().isResolving, false);
    assert.equal(view.last().anchorMessageId, "unread");
  } finally {
    view.unmount();
  }
});

const { UNREAD_SEEK_PAGE_SIZE, UNREAD_SEEK_DEADLINE_MS, UNREAD_SEEK_RETRY_MS } =
  await import("./useChannelOpenAnchor.ts");
const unreadOnly = [message("u1", 200), message("u2", 210)];
const seekProps = (fetchOlder) => ({
  messages: unreadOnly,
  windowCommit: { at: Date.now(), newestRowAt: 210 },
  lastMessageAt: iso(210),
  frontierSeconds: 150,
  firstUnreadMessageId: "u1",
  hasOlderMessages: true,
  fetchOlder,
});

const wait = (ms) =>
  act(() => new Promise((resolve) => setTimeout(resolve, ms)));

test("a far unread is sought in the relay's largest pages; failures are not counted", async () => {
  const sizes = [];
  const answers = [false, true, true, true];
  const view = mount(
    seekProps(async (size) => {
      sizes.push(size);
      return answers.shift();
    }),
  );
  try {
    assert.equal(view.last().isResolving, true);
    await wait(UNREAD_SEEK_RETRY_MS + 100);
    assert.deepEqual(sizes, Array(4).fill(UNREAD_SEEK_PAGE_SIZE));
    // Three pages appended without finding the start: open at the newest.
    assert.equal(view.last().isResolving, false);
    assert.equal(view.last().anchorMessageId, null);
  } finally {
    view.unmount();
  }
});

test("a relay that fails at once is asked again only after a pause", async () => {
  let calls = 0;
  const view = mount(
    seekProps(async () => {
      calls += 1;
      return false;
    }),
  );
  try {
    await wait(UNREAD_SEEK_RETRY_MS * 4);
    // Without the pause this would be hundreds of requests.
    assert.ok(calls >= 2 && calls <= 5, `${calls} requests`);
    assert.equal(view.last().isResolving, true);
  } finally {
    view.unmount();
  }
});

test("a seek that outlasts its deadline opens at the newest message", async () => {
  const view = mount(seekProps(() => new Promise(() => {})));
  try {
    view.update({ isFetchingOlder: true });
    assert.equal(view.last().isResolving, true);
    await act(
      () =>
        new Promise((resolve) =>
          setTimeout(resolve, UNREAD_SEEK_DEADLINE_MS + 50),
        ),
    );
    assert.equal(view.last().isResolving, false);
    assert.equal(view.last().anchorMessageId, null);
  } finally {
    view.unmount();
  }
});

test("a visit waits while the read frontier is pending", () => {
  const view = mount({
    messages: rows,
    windowCommit: { at: Date.now(), newestRowAt: 200 },
    lastMessageAt: iso(200),
    frontierSeconds: null,
    frontierPending: true,
    firstUnreadMessageId: null,
  });
  try {
    assert.equal(view.last().isResolving, true);
    view.update({
      frontierSeconds: 150,
      frontierPending: false,
      firstUnreadMessageId: "unread",
    });
    assert.equal(view.last().isResolving, false);
    assert.equal(view.last().anchorMessageId, "unread");
  } finally {
    view.unmount();
  }
});

test("a read marker from another device keeps where the visit opened", () => {
  const view = mount({
    messages: rows,
    windowCommit: { at: Date.now(), newestRowAt: 200 },
    lastMessageAt: iso(200),
    frontierSeconds: 150,
    firstUnreadMessageId: "unread",
  });
  try {
    assert.equal(view.last().anchorMessageId, "unread");
    // Another device reads everything while the room is open.
    view.update({ firstUnreadMessageId: null, frontierSeconds: 200 });
    assert.equal(view.last().isResolving, false);
    assert.equal(view.last().anchorMessageId, "unread");
  } finally {
    view.unmount();
  }
});

test("a commit announced before its rows reach the timeline waits for them", () => {
  const view = mount({
    messages: [message("old", 100)],
    windowCommit: { at: Date.now(), newestRowAt: 200, newestRowId: "unread" },
    lastMessageAt: iso(200),
    frontierSeconds: 150,
    firstUnreadMessageId: null,
  });
  try {
    assert.equal(view.last().isResolving, true);
    view.update({ messages: rows, firstUnreadMessageId: "unread" });
    assert.equal(view.last().isResolving, false);
    assert.equal(view.last().anchorMessageId, "unread");
  } finally {
    view.unmount();
  }
});

test("a frontier taken without the read state trusts no position", () => {
  window.localStorage.clear();
  const scope = readingPositionScope("a".repeat(64), "wss://relay.example");
  readingPositionStore.set(scope, "room", { messageId: "old", topPx: 30 });
  const view = mount({
    positionScope: scope,
    messages: rows,
    windowCommit: { at: Date.now(), newestRowAt: 200 },
    lastMessageAt: iso(200),
    frontierSeconds: 200,
    frontierUnknown: true,
    firstUnreadMessageId: null,
  });
  try {
    assert.equal(view.last().anchorMessageId, null);
    // Falling back keeps the spot for the next visit.
    assert.deepEqual(readingPositionStore.get(scope, "room"), {
      messageId: "old",
      topPx: 30,
    });
    // Settling at the newest does not forget it either.
    view.last().savePosition(null);
    assert.deepEqual(readingPositionStore.get(scope, "room"), {
      messageId: "old",
      topPx: 30,
    });
  } finally {
    view.unmount();
  }
});

test("a remembered row that is not there any more is forgotten", () => {
  window.localStorage.clear();
  const scope = readingPositionScope("a".repeat(64), "wss://relay.example");
  readingPositionStore.set(scope, "room", { messageId: "gone", topPx: 30 });
  const view = mount({
    positionScope: scope,
    messages: rows,
    windowCommit: { at: Date.now(), newestRowAt: 200 },
    lastMessageAt: iso(200),
    frontierSeconds: 200,
    firstUnreadMessageId: null,
  });
  try {
    assert.equal(view.last().anchorMessageId, null);
    assert.equal(readingPositionStore.get(scope, "room"), null);
  } finally {
    view.unmount();
  }
});
