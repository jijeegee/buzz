import assert from "node:assert/strict";
import { after, afterEach, beforeEach, test } from "node:test";
import { JSDOM } from "jsdom";

const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://localhost",
});
const invocations = [];
let sendHandler = () => Promise.reject(new Error("send not stubbed"));
dom.window.__TAURI_INTERNALS__ = {
  invoke: (command, args) => {
    if (command === "delete_message") return Promise.resolve(null);
    if (command !== "send_channel_message") {
      return Promise.reject(new Error(`unmocked Tauri command: ${command}`));
    }
    invocations.push(args);
    return sendHandler(args);
  },
  transformCallback: () => Math.random(),
};
Object.assign(globalThis, {
  document: dom.window.document,
  window: dom.window,
  localStorage: dom.window.localStorage,
  HTMLElement: dom.window.HTMLElement,
  IS_REACT_ACT_ENVIRONMENT: true,
  __TAURI_INTERNALS__: dom.window.__TAURI_INTERNALS__,
});

const React = await import("react");
const { act, cleanup, renderHook } = await import("@testing-library/react");
const { QueryClient, QueryClientProvider } = await import(
  "@tanstack/react-query"
);
const { useDeleteForumPostMutation, useForumLocalSends, useForumSend } =
  await import("./hooks.ts");
const { mergeForumPosts, mergeForumReplies, syncForumLocalSends } =
  await import("./lib/localSends.ts");
const { discardFailedSend, hasFailedSend, resetFailedSends, retryFailedSend } =
  await import("../messages/lib/failedSends.ts");
const { isStoredSendInFlight, resetStoredSendSession, storedSends } =
  await import("../messages/lib/failedSendStore.ts");
const { initDraftStore } = await import("../messages/lib/useDrafts.ts");
const { sendTimeOfDrawnRow } = await import("../messages/hooks.ts");

const CHANNEL = "11111111-1111-4111-8111-111111111111";
const ME = "a".repeat(64);
const POST = "d".repeat(64);
const KIND_POST = 45001;
const KIND_COMMENT = 45003;

function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

function fetchedPost(eventId, content, createdAt, extra = {}) {
  return {
    eventId,
    pubkey: ME,
    content,
    kind: KIND_POST,
    createdAt,
    channelId: CHANNEL,
    tags: [["h", CHANNEL]],
    threadSummary: null,
    ...extra,
  };
}

function fetchedReply(eventId, content, createdAt) {
  return {
    eventId,
    pubkey: ME,
    content,
    kind: KIND_COMMENT,
    createdAt,
    channelId: CHANNEL,
    tags: [
      ["h", CHANNEL],
      ["e", POST, "", "reply"],
    ],
    parentEventId: POST,
    rootEventId: POST,
    depth: 1,
  };
}

// No garbage-collection timers: they would keep the test process alive.
function newClient() {
  return new QueryClient({
    defaultOptions: {
      queries: { gcTime: Number.POSITIVE_INFINITY, retry: false },
      mutations: { gcTime: Number.POSITIVE_INFINITY, retry: false },
    },
  });
}

/** The posts list and the open thread, as the forum queries cache them. */
function seedForum(queryClient, posts = [], replies = []) {
  queryClient.setQueryData(["forum-posts", CHANNEL, null], {
    posts,
    nextCursor: null,
  });
  queryClient.setQueryData(["forum-thread", CHANNEL, POST, null], {
    post: fetchedPost(POST, "root", 100),
    replies,
    totalReplies: replies.length,
    nextCursor: null,
  });
}

function shownPosts(queryClient, rows) {
  return mergeForumPosts(
    queryClient.getQueryData(["forum-posts", CHANNEL, null]).posts,
    rows,
  );
}

function shownReplies(queryClient, rows) {
  return mergeForumReplies(
    queryClient.getQueryData(["forum-thread", CHANNEL, POST, null]).replies,
    rows,
    POST,
  );
}

function mount(queryClient) {
  const wrapper = ({ children }) =>
    React.createElement(QueryClientProvider, { client: queryClient }, children);
  return renderHook(
    () => {
      const forumSend = useForumSend(ME);
      const rows = useForumLocalSends(CHANNEL, forumSend.retry);
      return { forumSend, rows };
    },
    { wrapper },
  );
}

const flush = () => act(async () => {});

/** Own rows as `useForumLocalSends` returns them, read without a render. */
function localRows(queryClient) {
  return queryClient.getQueryData(["forum-local-sends", CHANNEL]) ?? [];
}

function post(content) {
  return {
    channelId: CHANNEL,
    kind: KIND_POST,
    content,
    parentEventId: null,
    mentionPubkeys: [],
    mediaTags: [],
  };
}

function accepted(args, id) {
  return Promise.resolve({
    event_id: id,
    parent_event_id: args.parentEventId ?? null,
    root_event_id: args.parentEventId ?? null,
    depth: args.parentEventId ? 1 : 0,
    created_at: args.createdAt,
  });
}

/** A fresh app start: same account on disk, nothing in memory. */
function restart() {
  resetFailedSends();
  resetStoredSendSession();
}

beforeEach(() => {
  localStorage.clear();
  initDraftStore(ME, "wss://forum.test");
  restart();
  invocations.length = 0;
  sendHandler = () => Promise.reject(new Error("send not stubbed"));
});
afterEach(() => cleanup());
after(() => dom.window.close());

test("an own post shows at once, and only once when the refetch brings it", async () => {
  const queryClient = newClient();
  seedForum(queryClient, [fetchedPost("old".padEnd(64, "0"), "older", 100)]);
  const gate = deferred();
  sendHandler = () => gate.promise;
  const view = mount(queryClient);

  act(() => view.result.current.forumSend.send(post("hello")));
  const [row] = localRows(queryClient);
  // Drawn before the relay answered, newest first, with a clock.
  assert.deepEqual(
    shownPosts(queryClient, localRows(queryClient)).map((p) => p.content),
    ["hello", "older"],
  );
  assert.equal(
    shownPosts(queryClient, localRows(queryClient))[0].pending,
    true,
  );
  await flush();
  // Signed at the second the row shows, so it does not move.
  assert.equal(invocations.length, 1);
  assert.equal(invocations[0].createdAt, row.event.created_at);
  assert.equal(invocations[0].kind, KIND_POST);
  assert.equal(storedSends(CHANNEL).length, 1);

  const id = "e1".padEnd(64, "0");
  await act(async () => gate.resolve(await accepted(invocations[0], id)));
  const sent = shownPosts(queryClient, localRows(queryClient))[0];
  assert.equal(sent.eventId, id);
  assert.equal(sent.pending, false);
  assert.equal(sent.localKey, row.localKey);
  assert.deepEqual(storedSends(CHANNEL), []);

  act(() =>
    seedForum(queryClient, [
      fetchedPost(id, "hello", row.event.created_at),
      fetchedPost("old".padEnd(64, "0"), "older", 100),
    ]),
  );
  assert.deepEqual(localRows(queryClient), []);
  assert.deepEqual(
    shownPosts(queryClient, localRows(queryClient)).map((p) => p.content),
    ["hello", "older"],
  );
});

test("an own reply shows at once at the end of its thread", async () => {
  const queryClient = newClient();
  seedForum(
    queryClient,
    [],
    [fetchedReply("r1".padEnd(64, "0"), "first", 100)],
  );
  sendHandler = () => new Promise(() => {});
  const view = mount(queryClient);
  act(() =>
    view.result.current.forumSend.send({
      ...post("second"),
      kind: KIND_COMMENT,
      parentEventId: POST,
    }),
  );
  const replies = shownReplies(queryClient, localRows(queryClient));
  assert.deepEqual(
    replies.map((reply) => reply.content),
    ["first", "second"],
  );
  assert.equal(replies[1].pending, true);
  await flush();
  assert.equal(invocations[0].parentEventId, POST);
  assert.equal(invocations[0].kind, KIND_COMMENT);
});

test("a failed post stays with Retry, which sends it again under its row", async () => {
  const queryClient = newClient();
  seedForum(queryClient);
  sendHandler = () => Promise.reject(new Error("offline"));
  const view = mount(queryClient);
  act(() => view.result.current.forumSend.send(post("hello")));
  await flush();
  const [row] = localRows(queryClient);
  assert.equal(row.failed, true);
  assert.equal(hasFailedSend(row.localKey), true);
  assert.equal(isStoredSendInFlight(row.localKey), false);
  assert.equal(storedSends(CHANNEL).length, 1);
  const failed = shownPosts(queryClient, localRows(queryClient))[0];
  assert.equal(failed.failed, true);
  assert.equal(failed.pending, true);

  const id = "e2".padEnd(64, "0");
  sendHandler = (args) => accepted(args, id);
  await act(async () => retryFailedSend(row.localKey));
  assert.equal(invocations.length, 2);
  assert.equal(invocations[1].content, "hello");
  assert.equal(invocations[1].createdAt, row.event.created_at);
  assert.equal(localRows(queryClient).length, 1);
  assert.equal(localRows(queryClient)[0].localKey, row.localKey);
  assert.equal(localRows(queryClient)[0].eventId, id);
  assert.equal(localRows(queryClient)[0].failed, false);
  assert.deepEqual(storedSends(CHANNEL), []);
});

test("Delete removes a failed post and its stored copy", async () => {
  const queryClient = newClient();
  seedForum(queryClient);
  sendHandler = () => Promise.reject(new Error("offline"));
  const view = mount(queryClient);
  act(() => view.result.current.forumSend.send(post("hello")));
  await flush();
  const [row] = localRows(queryClient);
  act(() => discardFailedSend(row.localKey));
  assert.deepEqual(localRows(queryClient), []);
  assert.deepEqual(storedSends(CHANNEL), []);
  assert.equal(invocations.length, 1);
});

test("a send that timed out after the relay stored it is not shown as failed", async () => {
  const queryClient = newClient();
  seedForum(queryClient);
  const gate = deferred();
  sendHandler = () => gate.promise;
  const view = mount(queryClient);
  act(() => view.result.current.forumSend.send(post("hello")));
  await flush();
  const [row] = localRows(queryClient);
  // A poll brings the relay's copy before the send's answer.
  act(() =>
    seedForum(queryClient, [
      fetchedPost("e3".padEnd(64, "0"), "hello", row.event.created_at),
    ]),
  );
  await act(async () => gate.reject(new Error("timed out")));
  assert.deepEqual(localRows(queryClient), []);
  assert.equal(hasFailedSend(row.localKey), false);
  assert.deepEqual(storedSends(CHANNEL), []);
  assert.equal(shownPosts(queryClient, []).length, 1);
});

/** A failed send left on disk by an earlier app session. */
async function failInEarlierSession(variables) {
  const queryClient = newClient();
  seedForum(queryClient);
  sendHandler = () => Promise.reject(new Error("offline"));
  const view = mount(queryClient);
  act(() => view.result.current.forumSend.send(variables));
  await flush();
  const [row] = localRows(queryClient);
  view.unmount();
  restart();
  invocations.length = 0;
  return row;
}

function relayCheck(lookup) {
  return {
    state: new Map(),
    lookup,
    onSettled: () => {},
    isDisposed: () => false,
  };
}

test("a failed post comes back after a restart, and Retry posts it", async () => {
  const row = await failInEarlierSession(post("kept"));
  const queryClient = newClient();
  seedForum(queryClient);
  const check = relayCheck(async () => []);
  const retried = [];
  syncForumLocalSends(
    queryClient,
    CHANNEL,
    async (variables) => retried.push(variables),
    check,
  );
  // Not drawn while the relay is asked whether it landed.
  assert.equal(
    queryClient.getQueryData(["forum-local-sends", CHANNEL]),
    undefined,
  );
  await new Promise((resolve) => setImmediate(resolve));
  syncForumLocalSends(
    queryClient,
    CHANNEL,
    async (variables) => retried.push(variables),
    check,
  );
  const rows = queryClient.getQueryData(["forum-local-sends", CHANNEL]);
  const shown = shownPosts(queryClient, rows);
  assert.equal(shown.length, 1);
  assert.equal(shown[0].content, "kept");
  assert.equal(shown[0].failed, true);
  assert.equal(shown[0].createdAt, row.event.created_at);
  assert.equal(hasFailedSend(row.localKey), true);

  retryFailedSend(row.localKey);
  assert.equal(isStoredSendInFlight(row.localKey), true);
  assert.deepEqual(retried, [
    {
      localKey: row.localKey,
      channelId: CHANNEL,
      kind: KIND_POST,
      content: "kept",
      parentEventId: null,
      mentionPubkeys: [],
      mediaTags: [],
    },
  ]);
});

test("a failed reply comes back into its thread after a restart", async () => {
  const row = await failInEarlierSession({
    ...post("reply kept"),
    kind: KIND_COMMENT,
    parentEventId: POST,
  });
  const queryClient = newClient();
  seedForum(queryClient);
  syncForumLocalSends(queryClient, CHANNEL, async () => {});
  const rows = queryClient.getQueryData(["forum-local-sends", CHANNEL]);
  const replies = shownReplies(queryClient, rows);
  assert.equal(replies.length, 1);
  assert.equal(replies[0].content, "reply kept");
  assert.equal(replies[0].failed, true);
  assert.equal(replies[0].localKey, row.localKey);
  assert.deepEqual(shownPosts(queryClient, rows), []);
});

function parentCheck(lookup) {
  return { state: new Map(), lookup, onSettled: () => {} };
}

/**
 * A reply that failed in an earlier session, synced after a restart with only
 * the posts list loaded (its thread closed) until both lookups settle.
 */
async function restoreReplyWithPostsList(posts, lookupParent) {
  const row = await failInEarlierSession({
    ...post("orphan"),
    kind: KIND_COMMENT,
    parentEventId: POST,
  });
  const queryClient = newClient();
  queryClient.setQueryData(["forum-posts", CHANNEL, null], {
    posts,
    nextCursor: null,
  });
  const relay = relayCheck(async () => []);
  const parents = parentCheck(lookupParent);
  for (let i = 0; i < 3; i++) {
    syncForumLocalSends(queryClient, CHANNEL, async () => {}, relay, parents);
    await new Promise((resolve) => setImmediate(resolve));
  }
  return { row, rows: localRows(queryClient) };
}

test("a failed reply to a deleted post is dropped after a restart", async () => {
  const lookups = [];
  const { row, rows } = await restoreReplyWithPostsList([], async (...args) => {
    lookups.push(args);
    return [];
  });
  assert.deepEqual(lookups, [[CHANNEL, POST]]);
  assert.deepEqual(rows, []);
  assert.equal(hasFailedSend(row.localKey), false);
  assert.deepEqual(storedSends(CHANNEL), []);
});

test("a failed reply to a listed post is restored without a lookup", async () => {
  const lookups = [];
  const { row, rows } = await restoreReplyWithPostsList(
    [fetchedPost(POST, "root", 100)],
    async (...args) => {
      lookups.push(args);
      return [];
    },
  );
  assert.deepEqual(lookups, []);
  assert.deepEqual(
    rows.map((send) => [send.localKey, send.failed]),
    [[row.localKey, true]],
  );
});

test("a failed reply is kept when its post is found or cannot be looked up", async () => {
  for (const lookupParent of [
    async () => [{ ...fetchedReply(POST, "root", 100), id: POST }],
    async () => {
      throw new Error("relay unreachable");
    },
  ]) {
    localStorage.clear();
    initDraftStore(ME, "wss://forum.test");
    restart();
    const { row, rows } = await restoreReplyWithPostsList([], lookupParent);
    assert.deepEqual(
      rows.map((send) => [send.localKey, send.failed]),
      [[row.localKey, true]],
    );
    assert.equal(hasFailedSend(row.localKey), true);
    assert.equal(storedSends(CHANNEL).length, 1);
  }
});

test("a post the relay stored before the app quit is not shown as failed", async () => {
  const row = await failInEarlierSession(post("landed"));
  const queryClient = newClient();
  seedForum(queryClient);
  const check = relayCheck(async () => [
    {
      id: "e4".padEnd(64, "0"),
      pubkey: ME,
      created_at: row.event.created_at,
      kind: KIND_POST,
      tags: [["h", CHANNEL]],
      content: "landed",
      sig: "",
    },
  ]);
  syncForumLocalSends(queryClient, CHANNEL, async () => {}, check);
  await new Promise((resolve) => setImmediate(resolve));
  syncForumLocalSends(queryClient, CHANNEL, async () => {}, check);
  assert.deepEqual(
    queryClient.getQueryData(["forum-local-sends", CHANNEL]) ?? [],
    [],
  );
  assert.equal(hasFailedSend(row.localKey), false);
  assert.deepEqual(storedSends(CHANNEL), []);
});

test("a post already in the loaded list is not restored as failed", async () => {
  const row = await failInEarlierSession(post("landed"));
  const queryClient = newClient();
  seedForum(queryClient, [
    fetchedPost("e5".padEnd(64, "0"), "landed", row.event.created_at),
  ]);
  syncForumLocalSends(queryClient, CHANNEL, async () => {});
  assert.equal(shownPosts(queryClient, []).length, 1);
  assert.equal(hasFailedSend(row.localKey), false);
  assert.deepEqual(storedSends(CHANNEL), []);
});

test("Retry after an unreachable relay check asks again and never posts twice", async () => {
  const row = await failInEarlierSession(post("maybe"));
  const queryClient = newClient();
  seedForum(queryClient);
  let landed = false;
  const check = relayCheck(async () => {
    if (!landed) throw new Error("offline");
    return [
      {
        id: "e6".padEnd(64, "0"),
        pubkey: ME,
        created_at: row.event.created_at,
        kind: KIND_POST,
        tags: [["h", CHANNEL]],
        content: "maybe",
        sig: "",
      },
    ];
  });
  const retried = [];
  const retry = async (variables) => retried.push(variables);
  syncForumLocalSends(queryClient, CHANNEL, retry, check);
  await new Promise((resolve) => setImmediate(resolve));
  syncForumLocalSends(queryClient, CHANNEL, retry, check);
  assert.equal(check.state.get(row.localKey), "unverified");
  assert.equal(hasFailedSend(row.localKey), true);

  landed = true;
  retryFailedSend(row.localKey);
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(retried, []);
  assert.deepEqual(
    queryClient.getQueryData(["forum-local-sends", CHANNEL]),
    [],
  );
  assert.deepEqual(storedSends(CHANNEL), []);
});

test("an earlier post with the same text does not count as delivered", async () => {
  const row = await failInEarlierSession(post("ok"));
  const queryClient = newClient();
  seedForum(queryClient, [
    fetchedPost("e7".padEnd(64, "0"), "ok", row.event.created_at - 30),
  ]);
  syncForumLocalSends(queryClient, CHANNEL, async () => {});
  assert.equal(hasFailedSend(row.localKey), true);
  assert.equal(storedSends(CHANNEL).length, 1);
  assert.equal(
    shownPosts(
      queryClient,
      queryClient.getQueryData(["forum-local-sends", CHANNEL]),
    ).length,
    2,
  );
});

test("deleting a just-sent post no fetch has returned yet removes its row", async () => {
  const queryClient = newClient();
  seedForum(queryClient);
  const id = "e8".padEnd(64, "0");
  sendHandler = (args) => accepted(args, id);
  const wrapper = ({ children }) =>
    React.createElement(QueryClientProvider, { client: queryClient }, children);
  const view = renderHook(
    () => {
      const forumSend = useForumSend(ME);
      useForumLocalSends(CHANNEL, forumSend.retry);
      return {
        forumSend,
        deletePost: useDeleteForumPostMutation({
          id: CHANNEL,
          channelType: "forum",
        }),
      };
    },
    { wrapper },
  );
  act(() => view.result.current.forumSend.send(post("oops")));
  await flush();
  assert.equal(localRows(queryClient)[0].eventId, id);
  await act(async () =>
    view.result.current.deletePost.mutateAsync({ eventId: id }),
  );
  assert.deepEqual(localRows(queryClient), []);
  assert.deepEqual(shownPosts(queryClient, localRows(queryClient)), []);
});

/** Run `body` with the clock at `at` (ms), restoring it afterwards. */
async function atTime(at, body) {
  const realNow = Date.now;
  Date.now = () => at;
  try {
    return await body();
  } finally {
    Date.now = realNow;
  }
}

test("a send keeps its drawn second only while the backend still signs at it", () => {
  assert.equal(sendTimeOfDrawnRow(1_000, 1_000), 1_000);
  assert.equal(sendTimeOfDrawnRow(1_000, 1_110), 1_000);
  assert.equal(sendTimeOfDrawnRow(1_000, 1_111), 1_111);
  // A row drawn "in the future" (the clock went back) is signed now.
  assert.equal(sendTimeOfDrawnRow(1_200, 1_100), 1_100);
});

test("Retry more than two minutes later redraws the row at the second it is signed", async () => {
  const queryClient = newClient();
  seedForum(queryClient);
  sendHandler = () => Promise.reject(new Error("offline"));
  const view = mount(queryClient);
  const start = Date.now();
  await atTime(start, async () => {
    act(() => view.result.current.forumSend.send(post("late")));
    await flush();
  });
  const [row] = localRows(queryClient);

  await atTime(start + 200_000, () =>
    act(async () => retryFailedSend(row.localKey)),
  );
  assert.equal(invocations.length, 2);
  const [redrawn] = localRows(queryClient);
  assert.equal(redrawn.localKey, row.localKey);
  assert.equal(redrawn.failed, true);
  assert.equal(redrawn.event.created_at, Math.floor((start + 200_000) / 1000));
  // Row, signing time and disk copy all carry the new second.
  assert.equal(invocations[1].createdAt, redrawn.event.created_at);
  assert.equal(
    storedSends(CHANNEL)[0].event.created_at,
    redrawn.event.created_at,
  );

  // The relay's copy, signed then, is recognised: no stuck "Not sent" row.
  act(() =>
    seedForum(queryClient, [
      fetchedPost("e9".padEnd(64, "0"), "late", redrawn.event.created_at),
    ]),
  );
  assert.deepEqual(localRows(queryClient), []);
  assert.equal(hasFailedSend(row.localKey), false);
  assert.deepEqual(storedSends(CHANNEL), []);
});

test("Retry within two minutes keeps the row's second", async () => {
  const queryClient = newClient();
  seedForum(queryClient);
  sendHandler = () => Promise.reject(new Error("offline"));
  const view = mount(queryClient);
  const start = Date.now();
  await atTime(start, async () => {
    act(() => view.result.current.forumSend.send(post("soon")));
    await flush();
  });
  const [row] = localRows(queryClient);
  await atTime(start + 60_000, () =>
    act(async () => retryFailedSend(row.localKey)),
  );
  assert.equal(invocations[1].createdAt, row.event.created_at);
  assert.equal(
    localRows(queryClient)[0].event.created_at,
    row.event.created_at,
  );
});

function localReply(localKey, content, createdAt) {
  return {
    localKey,
    channelId: CHANNEL,
    parentEventId: POST,
    event: {
      id: localKey,
      localKey,
      pubkey: ME,
      created_at: createdAt,
      kind: KIND_COMMENT,
      tags: [
        ["h", CHANNEL],
        ["e", POST, "", "reply"],
      ],
      content,
      sig: "",
      pending: true,
    },
    eventId: null,
    failed: false,
  };
}

test("replies read oldest first whatever order the relay returns them in", () => {
  // The relay answers newest first.
  const fetched = [
    fetchedReply("r3".padEnd(64, "0"), "third", 300),
    fetchedReply("r2".padEnd(64, "0"), "second", 200),
    fetchedReply("r1".padEnd(64, "0"), "first", 100),
  ];
  assert.deepEqual(
    mergeForumReplies(fetched, [], POST).map((reply) => reply.content),
    ["first", "second", "third"],
  );
  assert.deepEqual(
    mergeForumReplies(
      fetched,
      [
        localReply("forum-local-late", "mine, latest", 400),
        localReply("forum-local-mid", "mine, between", 250),
        localReply("forum-local-tie", "mine, same second", 200),
      ],
      POST,
    ).map((reply) => reply.content),
    [
      "first",
      "second",
      "mine, same second",
      "mine, between",
      "third",
      "mine, latest",
    ],
  );
});

const sameReply = { ...post("same"), kind: KIND_COMMENT, parentEventId: POST };

test("one copy settles only the acknowledged one of two identical replies of a second", async () => {
  const queryClient = newClient();
  seedForum(queryClient);
  const gates = [deferred(), deferred()];
  sendHandler = () => gates[invocations.length - 1].promise;
  const view = mount(queryClient);
  await atTime(Date.now(), async () => {
    act(() => view.result.current.forumSend.send(sameReply));
    act(() => view.result.current.forumSend.send(sameReply));
    await flush();
  });
  const [first, second] = localRows(queryClient);
  assert.equal(first.event.created_at, second.event.created_at);

  const id = "f1".padEnd(64, "0");
  await act(async () => gates[0].resolve(await accepted(invocations[0], id)));
  act(() =>
    seedForum(
      queryClient,
      [],
      [fetchedReply(id, "same", first.event.created_at)],
    ),
  );
  assert.deepEqual(
    localRows(queryClient).map((row) => row.localKey),
    [second.localKey],
  );

  // The second fails: the first's copy does not pass for it.
  await act(async () => gates[1].reject(new Error("offline")));
  assert.equal(localRows(queryClient)[0].failed, true);
  assert.equal(hasFailedSend(second.localKey), true);
  assert.equal(storedSends(CHANNEL).length, 1);
  assert.deepEqual(
    shownReplies(queryClient, localRows(queryClient)).map((reply) => [
      reply.content,
      reply.failed === true,
    ]),
    [
      ["same", false],
      ["same", true],
    ],
  );
});

test("one copy before either answer settles one of two identical replies", async () => {
  const queryClient = newClient();
  seedForum(queryClient);
  const gates = [deferred(), deferred()];
  sendHandler = () => gates[invocations.length - 1].promise;
  const view = mount(queryClient);
  await atTime(Date.now(), async () => {
    act(() => view.result.current.forumSend.send(sameReply));
    act(() => view.result.current.forumSend.send(sameReply));
    await flush();
  });
  const [first, second] = localRows(queryClient);
  const id = "f2".padEnd(64, "0");
  const copy = [fetchedReply(id, "same", first.event.created_at)];
  act(() => seedForum(queryClient, [], copy));
  assert.equal(localRows(queryClient).length, 1);
  // Later polls still leave one row for the one copy.
  act(() => seedForum(queryClient, [], [...copy]));
  assert.equal(localRows(queryClient).length, 1);

  await act(async () => gates[0].resolve(await accepted(invocations[0], id)));
  await act(async () => gates[1].reject(new Error("offline")));
  const [left] = localRows(queryClient);
  assert.equal(left.localKey, second.localKey);
  assert.equal(left.failed, true);
  assert.equal(hasFailedSend(second.localKey), true);
  assert.equal(storedSends(CHANNEL).length, 1);
});

test("one loaded copy settles only one of two identical stored sends", async () => {
  // Two identical sends of the same second, both left on disk.
  const earlier = newClient();
  seedForum(earlier);
  sendHandler = () => Promise.reject(new Error("offline"));
  const view = mount(earlier);
  await atTime(Date.now(), async () => {
    act(() => view.result.current.forumSend.send(sameReply));
    act(() => view.result.current.forumSend.send(sameReply));
    await flush();
  });
  view.unmount();
  restart();
  assert.equal(storedSends(CHANNEL).length, 2);
  const createdAt = storedSends(CHANNEL)[0].event.created_at;
  const queryClient = newClient();
  seedForum(
    queryClient,
    [],
    [fetchedReply("f3".padEnd(64, "0"), "same", createdAt)],
  );
  syncForumLocalSends(queryClient, CHANNEL, async () => {});
  assert.equal(storedSends(CHANNEL).length, 1);
  const rows = queryClient.getQueryData(["forum-local-sends", CHANNEL]);
  assert.equal(rows.length, 1);
  assert.equal(rows[0].failed, true);
});
