/**
 * A thread panel opened on its root id never closes for a missing root
 * (docs/message-sync.md#opening-a-thread-from-its-root-id); forum and huddle
 * panels keep closing when their head is not loaded.
 */
import assert from "node:assert/strict";
import { test } from "node:test";

import React, { act } from "react";
import { createRoot } from "react-dom/client";

import { useThreadTargetSync } from "./useThreadTargetSync.ts";

const ROOT = "r".repeat(64);

async function mount(props) {
  const calls = { closed: 0, replyTargets: [], renders: 0 };
  function Harness(extra) {
    calls.renders += 1;
    const [replyTargetId, setReplyTargetId] = React.useState(null);
    useThreadTargetSync({
      clearOptimisticThreadOverride: () => {},
      editTargetId: null,
      editTargetMessage: null,
      isTimelineLoading: false,
      openThreadHeadId: ROOT,
      setEditTargetId: () => {},
      setExpandedThreadReplyIds: () => {},
      setOpenThreadHeadId: (value) => {
        if (value === null) calls.closed += 1;
      },
      setThreadReplyTargetId: (value) => {
        calls.replyTargets.push(value);
        setReplyTargetId(value);
      },
      setThreadScrollTargetId: () => {},
      threadHeadId: ROOT,
      threadReplyTargetId: replyTargetId,
      threadReplyTargetMessage: null,
      ...extra,
    });
    return null;
  }
  const root = createRoot(document.createElement("div"));
  await act(async () => root.render(React.createElement(Harness, props)));
  return { calls, unmount: () => act(async () => root.unmount()) };
}

test("a root not found yet keeps the panel open and targets the root", async () => {
  const { calls, unmount } = await mount({
    opensOnRootId: true,
    threadHeadMessage: null,
  });
  assert.equal(calls.closed, 0);
  assert.deepEqual(calls.replyTargets, [ROOT]);
  assert.ok(calls.renders <= 3, `settles in ${calls.renders} renders`);
  await unmount();
});

test("a placeholder head settles the reply target on the root", async () => {
  const { calls, unmount } = await mount({
    opensOnRootId: true,
    threadHeadMessage: { id: ROOT, rootPlaceholder: true },
  });
  assert.equal(calls.closed, 0);
  assert.deepEqual(calls.replyTargets, [ROOT]);
  assert.ok(calls.renders <= 3, `settles in ${calls.renders} renders`);
  await unmount();
});

test("a forum or huddle panel still closes when its head is not loaded", async () => {
  const { calls, unmount } = await mount({
    opensOnRootId: false,
    threadHeadMessage: null,
  });
  assert.equal(calls.closed, 1);
  assert.deepEqual(calls.replyTargets, []);
  await unmount();
});
