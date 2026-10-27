import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { JSDOM } from "jsdom";
import { HOME_MENTION_EVENT_KINDS } from "@/shared/constants/kinds";

const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://localhost",
});
before(() => {
  Object.assign(globalThis, {
    document: dom.window.document,
    HTMLElement: dom.window.HTMLElement,
    IS_REACT_ACT_ENVIRONMENT: true,
    window: dom.window,
    localStorage: dom.window.localStorage,
  });
});
after(() => dom.window.close());

const VIEWER = "a".repeat(64);
const PEER = "b".repeat(64);
function channels(count) {
  return Array.from({ length: count }, (_, i) => ({
    id: `channel-${i}`,
    name: `channel-${i}`,
    channelType: i === 0 ? "dm" : "stream",
  }));
}
function message(id, overrides = {}) {
  return {
    id,
    kind: 9,
    pubkey: PEER,
    content: "hello",
    created_at: Math.floor(Date.now() / 1000),
    tags: [
      ["h", "channel-0"],
      ["p", VIEWER],
    ],
    sig: "",
    ...overrides,
  };
}

async function mount(initialChannels, options = {}, subscribeImpl) {
  const { act, cleanup, renderHook } = await import("@testing-library/react");
  const React = await import("react");
  const { QueryClient, QueryClientProvider } = await import(
    "@tanstack/react-query"
  );
  const { relayClient } = await import("@/shared/api/relayClient");
  const { useLiveChannelUpdates } = await import("./useLiveChannelUpdates.ts");
  const { channelMessagesKey } = await import(
    "@/features/messages/lib/messageQueryKeys"
  );
  const originalLive = relayClient.subscribeLive;
  // Keep a tripwire for the removed API so restoring the second family fails.
  const originalMention = Object.getOwnPropertyDescriptor(
    relayClient,
    "subscribeToChannelMentionEvents",
  );
  const subscriptions = [];
  const mentionSubscriptions = [];
  relayClient.subscribeLive = async (filter, onEvent) => {
    const sub = { filter, onEvent, disposed: false };
    subscriptions.push(sub);
    await subscribeImpl?.(sub);
    return async () => {
      sub.disposed = true;
    };
  };
  relayClient.subscribeToChannelMentionEvents = async (...args) => {
    mentionSubscriptions.push(args);
    return async () => {};
  };
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: Infinity } },
  });
  const wrapper = ({ children }) =>
    React.createElement(QueryClientProvider, { client: queryClient }, children);
  const hook = renderHook(
    ({ members, opts }) => useLiveChannelUpdates(members, null, opts),
    {
      wrapper,
      initialProps: {
        members: initialChannels,
        opts: { currentPubkey: VIEWER, ...options },
      },
    },
  );
  const settle = () =>
    act(async () => {
      // Drain subscription setup and React Query notifications, not relay timing.
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
  await settle();
  return {
    act,
    settle,
    subscriptions,
    mentionSubscriptions,
    queryClient,
    channelMessagesKey,
    rerender(members, opts = options) {
      hook.rerender({ members, opts: { currentPubkey: VIEWER, ...opts } });
    },
    async deliver(sub, event) {
      await act(async () => {
        sub.onEvent(event);
      });
    },
    unmount: hook.unmount,
    restore() {
      hook.unmount();
      cleanup();
      queryClient.clear();
      // The bundle lives on the singleton client; forget this test's rooms
      // and delivered events.
      relayClient.disconnect();
      relayClient.subscribeLive = originalLive;
      if (originalMention) {
        Object.defineProperty(
          relayClient,
          "subscribeToChannelMentionEvents",
          originalMention,
        );
      } else {
        delete relayClient.subscribeToChannelMentionEvents;
      }
    },
  };
}

for (const count of [20, 50, 129]) {
  test(`${count} member channels share one live REQ per 128 rooms, not a second mention family`, async () => {
    const h = await mount(channels(count), { onLiveMention() {} });
    try {
      assert.equal(h.subscriptions.length, Math.ceil(count / 128));
      assert.equal(
        h.mentionSubscriptions.length,
        0,
        "channel streams already include mention kinds",
      );
      assert.deepEqual(
        h.subscriptions.flatMap((s) => s.filter["#h"]).sort(),
        channels(count)
          .map((c) => c.id)
          .sort(),
      );
      assert.ok(
        h.subscriptions.every(
          (s) => s.filter.limit === 0 && s.filter.since === undefined,
        ),
        "live-only, and no since that would drop backdated events",
      );
      assert.ok(
        h.subscriptions.every((s) =>
          HOME_MENTION_EVENT_KINDS.every((kind) =>
            s.filter.kinds.includes(kind),
          ),
        ),
        "the remaining wire filter must cover every Home mention kind",
      );
    } finally {
      h.restore();
    }
  });
}

test("live channel stream drives mention, unread and DM callbacks once across replay", async () => {
  const mentions = [];
  const unreads = [];
  const dms = [];
  const h = await mount(channels(1), {
    onLiveMention: () => mentions.push("mention"),
    onChannelMessage: (id, event) => unreads.push([id, event.id]),
    onDmMessage: (event, channel) => dms.push([channel.id, event.id]),
  });
  try {
    const event = message("mention");
    await h.deliver(h.subscriptions[0], event);
    await h.deliver(h.subscriptions[0], event);
    assert.deepEqual(mentions, ["mention"]);
    assert.deepEqual(unreads, [["channel-0", "mention"]]);
    assert.deepEqual(dms, [["channel-0", "mention"]]);
  } finally {
    h.restore();
  }
});

test("chat recency advances for actual self/peer messages, not metadata or auxiliary events", async () => {
  const { channelsQueryKey } = await import("./hooks.ts");
  const { sortChatList } = await import("@/features/sidebar/lib/chatList");
  const initial = channels(2).map((channel, index) => ({
    ...channel,
    lastMessageAt: new Date((100 + index) * 1000).toISOString(),
  }));
  const h = await mount(initial);
  try {
    h.queryClient.setQueryData(channelsQueryKey, initial);
    const order = () =>
      sortChatList(h.queryClient.getQueryData(channelsQueryKey)).map(
        ({ id }) => id,
      );
    assert.deepEqual(order(), ["channel-1", "channel-0"]);
    // Deliver through the production subscription callback even when the kind
    // would normally arrive on the separate metadata subscription.
    for (const kind of [39000, 39002, 7, 5, 40001]) {
      await h.deliver(
        h.subscriptions[0],
        message(`metadata-${kind}`, { kind, created_at: 200 }),
      );
    }
    assert.deepEqual(order(), ["channel-1", "channel-0"]);
    await h.deliver(
      h.subscriptions[0],
      message("self-message", { pubkey: VIEWER, created_at: 201 }),
    );
    assert.deepEqual(order(), ["channel-0", "channel-1"]);
    await h.deliver(
      h.subscriptions[0],
      message("peer-message", { created_at: 202, tags: [["h", "channel-1"]] }),
    );
    assert.deepEqual(order(), ["channel-1", "channel-0"]);
    assert.deepEqual(
      sortChatList(
        h.queryClient.getQueryData(channelsQueryKey),
        new Set(["channel-0"]),
      ).map(({ id }) => id),
      ["channel-0", "channel-1"],
    );
  } finally {
    h.restore();
  }
});

test("mention signal retains kind, recipient, self and member-channel boundaries", async () => {
  let mentions = 0;
  const h = await mount(channels(1), { onLiveMention: () => mentions++ });
  try {
    const sub = h.subscriptions[0];
    await h.deliver(
      sub,
      message("not-mentioned", { tags: [["h", "channel-0"]] }),
    );
    await h.deliver(sub, message("self", { pubkey: VIEWER.toUpperCase() }));
    for (const kind of [5, 7, 9005, 40001, 40003, 40008, 40099, 48100, 48101]) {
      await h.deliver(sub, message(`aux-${kind}`, { kind }));
    }
    await h.deliver(
      sub,
      message("outside", {
        tags: [
          ["h", "outside"],
          ["p", VIEWER],
        ],
      }),
    );
    assert.equal(mentions, 0);
    for (const kind of [9, 40002, 45001, 45003]) {
      await h.deliver(
        sub,
        message(`mention-${kind}`, {
          kind,
          tags: [
            ["h", "channel-0"],
            ["p", VIEWER.toUpperCase()],
          ],
        }),
      );
    }
    assert.equal(mentions, 4);
  } finally {
    h.restore();
  }
});

test("latest mention callback and identity are used without subscription churn", async () => {
  const notifications = [];
  const h = await mount(channels(1));
  try {
    const sub = h.subscriptions[0];
    await h.deliver(sub, message("before-callback"));
    h.rerender(channels(1), {
      onLiveMention: () => notifications.push("first"),
    });
    await h.deliver(sub, message("first"));
    h.rerender(channels(1), {
      onLiveMention: () => notifications.push("latest"),
    });
    await h.deliver(sub, message("second"));
    h.rerender(channels(1), {
      currentPubkey: "c".repeat(64),
      onLiveMention: () => notifications.push("new-identity"),
    });
    await h.deliver(sub, message("old-viewer"));
    await h.deliver(
      sub,
      message("new-viewer", {
        tags: [
          ["h", "channel-0"],
          ["p", "c".repeat(64)],
        ],
      }),
    );
    h.rerender(channels(1));
    await h.deliver(sub, message("disabled"));
    assert.deepEqual(notifications, ["first", "latest", "new-identity"]);
    assert.equal(h.subscriptions.length, 1);
    assert.equal(h.mentionSubscriptions.length, 0);
  } finally {
    h.restore();
  }
});

test("membership diff replaces the room bundle and drops a removed channel's late events", async () => {
  let mentions = 0;
  const opts = { onLiveMention: () => mentions++ };
  const h = await mount(channels(2), opts);
  try {
    const [previous] = h.subscriptions;
    h.rerender(channels(3).slice(1), opts);
    await h.settle();
    assert.equal(h.subscriptions.length, 2, "one replacement bundle");
    const [, replacement] = h.subscriptions;
    assert.deepEqual(replacement.filter["#h"], ["channel-1", "channel-2"]);
    assert.equal(previous.disposed, true, "retired once the successor is live");
    assert.equal(replacement.disposed, false);
    await h.deliver(previous, message("removed"));
    await h.deliver(
      replacement,
      message("retained", {
        tags: [
          ["h", "channel-1"],
          ["p", VIEWER],
        ],
      }),
    );
    assert.equal(
      mentions,
      1,
      "late event from a removed channel must not notify",
    );
  } finally {
    h.restore();
  }
});

test("untagged reaction finds its room from the loaded timeline and keeps that context", async () => {
  let mentions = 0;
  const h = await mount(channels(2), { onLiveMention: () => mentions++ });
  try {
    const key = h.channelMessagesKey("channel-1");
    h.queryClient.setQueryData(key, [
      message("parent", { tags: [["h", "channel-1"]] }),
    ]);
    await h.deliver(
      h.subscriptions[0],
      message("reaction", {
        kind: 7,
        content: "👀",
        tags: [
          ["e", "parent"],
          ["p", VIEWER],
        ],
      }),
    );
    const reaction = h.queryClient
      .getQueryData(key)
      .find((event) => event.id === "reaction");
    assert.ok(reaction);
    assert.deepEqual(reaction.tags.at(-1), ["h", "channel-1"]);
    assert.equal(mentions, 0);
  } finally {
    h.restore();
  }
});

test("a failed bundle setup retries the same rooms after backoff", async () => {
  const originalSetTimeout = window.setTimeout;
  const originalClearTimeout = window.clearTimeout;
  const retries = new Map();
  let timer = 0;
  window.setTimeout = (fn) => {
    retries.set(++timer, fn);
    return timer;
  };
  window.clearTimeout = (id) => retries.delete(id);
  let failOnce = true;
  const h = await mount(channels(2), {}, async () => {
    if (failOnce) {
      failOnce = false;
      throw new Error("fixture subscription setup failure");
    }
  });
  try {
    assert.equal(h.subscriptions.length, 1);
    assert.equal(retries.size, 1);
    await h.act(async () => {
      retries.values().next().value();
    });
    await h.settle();
    assert.equal(h.subscriptions.length, 2);
    assert.deepEqual(h.subscriptions[1].filter["#h"], [
      "channel-0",
      "channel-1",
    ]);
  } finally {
    h.restore();
    window.setTimeout = originalSetTimeout;
    window.clearTimeout = originalClearTimeout;
  }
});

test("unmount disposes a pending room bundle once it settles", async () => {
  let release;
  const h = await mount(channels(2), { onLiveMention() {} }, async () => {
    await new Promise((resolve) => {
      release = resolve;
    });
  });
  try {
    h.unmount();
    await h.act(async () => {
      release();
    });
    await h.settle();
    assert.ok(h.subscriptions.length > 0);
    assert.ok(h.subscriptions.every((sub) => sub.disposed));
    assert.equal(h.mentionSubscriptions.length, 0);
  } finally {
    h.restore();
  }
});

test("live messages patch the shown server chats list in place, once per event", async () => {
  const { chatListQueryKey } = await import("@/features/chats/chatListState");
  const { QueryObserver } = await import("@tanstack/react-query");
  const h = await mount(channels(2));
  const key = chatListQueryKey("wss://relay", VIEWER, []);
  const roomRow = (id, at, unread) => ({
    type: "room",
    key: id,
    at,
    at_us: at * 1_000_000,
    channel_id: id,
    channel_type: "stream",
    channel_name: id,
    archived: false,
    dm_members: [],
    root_id: null,
    thread_name: null,
    follow_state: null,
    closed: false,
    last_seq: 1,
    read_seq: 0,
    unread,
    unread_overflow: false,
    unread_mentions: 0,
    unread_mentions_overflow: false,
    unread_mark: false,
    latest: null,
    root: null,
  });
  const initial = {
    generation: 1,
    next: null,
    rows: [roomRow("channel-0", 200, 0), roomRow("channel-1", 100, 1)],
  };
  h.queryClient.setQueryData(key, initial);
  // The sidebar's query observer makes the list "shown".
  const observer = new QueryObserver(h.queryClient, {
    queryFn: async () => initial,
    queryKey: key,
    staleTime: Number.POSITIVE_INFINITY,
  });
  const unsubscribe = observer.subscribe(() => {});
  const mention = (id) =>
    message(id, {
      created_at: 300,
      tags: [
        ["h", "channel-1"],
        ["p", VIEWER],
      ],
    });
  try {
    await h.deliver(h.subscriptions[0], mention("peer-in-1"));
    let rows = h.queryClient.getQueryData(key).rows;
    assert.deepEqual(
      rows.map((row) => [row.key, row.unread, row.unread_mentions]),
      [
        ["channel-1", 2, 1],
        ["channel-0", 0, 0],
      ],
    );
    assert.equal(rows[0].latest.id, "peer-in-1");

    // A second message in the same second, then a repeat of the first
    // (reconnect replay): the repeat is not counted again.
    await h.deliver(h.subscriptions[0], mention("peer-in-2"));
    await h.deliver(h.subscriptions[0], mention("peer-in-1"));
    rows = h.queryClient.getQueryData(key).rows;
    assert.deepEqual(
      [rows[0].key, rows[0].unread, rows[0].unread_mentions],
      ["channel-1", 3, 2],
    );
  } finally {
    unsubscribe();
    h.restore();
  }
});
