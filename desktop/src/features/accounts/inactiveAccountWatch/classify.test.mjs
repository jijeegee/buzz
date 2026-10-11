import assert from "node:assert/strict";
import test from "node:test";

import { classifyWatchedEvent } from "./classify.ts";

const ME = "a".repeat(64);
const OTHER = "b".repeat(64);
const stream = { id: "ch", channelType: "stream" };
const dm = { id: "dm1", channelType: "dm" };

function context(overrides = {}) {
  return {
    pubkey: ME,
    settings: {
      desktopEnabled: true,
      slotAlertsEnabled: { dm: true, mention: true, thread_reply: true },
    },
    mutedChannelIds: new Set(),
    participatedRootIds: new Set(),
    followedRootIds: new Set(),
    authoredRootIds: new Set(),
    mutedRootIds: new Set(),
    ...overrides,
  };
}

function event({ pubkey = OTHER, kind = 9, tags = [] } = {}) {
  return { id: "e", pubkey, kind, tags, content: "hi", created_at: 1 };
}

const reply = (root) => [
  ["e", root, "", "root"],
  ["e", root, "", "reply"],
];

const cases = [
  [
    "DM → dm",
    event(),
    dm,
    context(),
    { notify: "dm", unread: true, mention: false },
  ],
  [
    "DM with dm slot off → badge only",
    event(),
    dm,
    context({
      settings: {
        desktopEnabled: true,
        slotAlertsEnabled: { dm: false, mention: true, thread_reply: true },
      },
    }),
    { notify: null, unread: true, mention: false },
  ],
  [
    "mention → mention",
    event({ tags: [["p", ME]] }),
    stream,
    context(),
    { notify: "mention", unread: true, mention: true },
  ],
  [
    "top-level channel message → badge only",
    event(),
    stream,
    context(),
    { notify: null, unread: true, mention: false },
  ],
  [
    "reply in a followed thread → thread_reply",
    event({ tags: reply("r1") }),
    stream,
    context({ followedRootIds: new Set(["r1"]) }),
    { notify: "thread_reply", unread: true, mention: false },
  ],
  [
    "reply in an unrelated thread → nothing",
    event({ tags: reply("r2") }),
    stream,
    context(),
    { notify: null, unread: false, mention: false },
  ],
  [
    "reply that mentions me → mention, not thread_reply",
    event({ tags: [...reply("r1"), ["p", ME]] }),
    stream,
    context({ participatedRootIds: new Set(["r1"]) }),
    { notify: "mention", unread: true, mention: true },
  ],
  [
    "own event → nothing",
    event({ pubkey: ME }),
    dm,
    context(),
    { notify: null, unread: false, mention: false },
  ],
  [
    "muted channel message → nothing",
    event(),
    stream,
    context({ mutedChannelIds: new Set(["ch"]) }),
    { notify: null, unread: false, mention: false },
  ],
  [
    "muted channel mention → still a mention",
    event({ tags: [["p", ME]] }),
    stream,
    context({ mutedChannelIds: new Set(["ch"]) }),
    { notify: "mention", unread: true, mention: true },
  ],
  [
    "desktop notifications off → badge only",
    event({ tags: [["p", ME]] }),
    stream,
    context({
      settings: {
        desktopEnabled: false,
        slotAlertsEnabled: { dm: true, mention: true, thread_reply: true },
      },
    }),
    { notify: null, unread: true, mention: true },
  ],
  [
    "reaction in a DM → nothing",
    event({ kind: 7 }),
    dm,
    context(),
    { notify: null, unread: false, mention: false },
  ],
];

for (const [name, input, channel, ctx, expected] of cases) {
  test(`classifyWatchedEvent: ${name}`, () => {
    assert.deepEqual(classifyWatchedEvent(input, channel, ctx), expected);
  });
}
