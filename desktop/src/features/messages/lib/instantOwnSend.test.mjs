import assert from "node:assert/strict";
import test from "node:test";
import { QueryClient } from "@tanstack/react-query";

import {
  createOptimisticMessage,
  removeOptimisticMessage,
  updateOptimisticMessage,
  writeOptimisticMessage,
} from "../hooks.ts";
import {
  emptyChannelWindowStore,
  replaceNewestChannelWindow,
} from "./channelWindowStore.ts";
import {
  discardFailedSend,
  hasFailedSend,
  registerFailedSend,
  replaceFailedSendRetry,
  resetFailedSends,
  retryFailedSend,
} from "./failedSends.ts";
import { reconcileIncomingMessage } from "./messageMerge.ts";
import {
  channelMessagesKey,
  channelWindowKey,
  threadRepliesKey,
} from "./messageQueryKeys.ts";
import { mergeNewestThreadPage } from "./threadWindowCache.ts";

const CHANNEL = "channel";
const ROOT = "r".repeat(64);
const identity = { pubkey: "a".repeat(64) };

function sent(id, createdAt, extraTags = []) {
  return {
    id: id.padEnd(64, "0"),
    pubkey: identity.pubkey,
    created_at: createdAt,
    kind: 9,
    tags: [["h", CHANNEL], ...extraTags],
    content: id,
    sig: "b".repeat(128),
  };
}

function harness() {
  const client = new QueryClient();
  const root = sent("root", 100);
  client.setQueryData(
    channelWindowKey(CHANNEL),
    replaceNewestChannelWindow(emptyChannelWindowStore(), {
      startCursor: null,
      rows: [{ event: root, thread: null }],
      aux: [],
      nextCursor: null,
      hasMore: false,
    }),
  );
  client.setQueryData(channelMessagesKey(CHANNEL), [root]);
  return { client, root };
}

function timeline(client) {
  return client.getQueryData(channelMessagesKey(CHANNEL));
}

function optimistic(content, localKey, parentEventId = null, existing = []) {
  return createOptimisticMessage(
    CHANNEL,
    content,
    identity,
    existing,
    [],
    parentEventId,
    [],
    null,
    null,
    localKey,
  );
}

test("a previewed own message is in the timeline before any send work", () => {
  const { client } = harness();
  writeOptimisticMessage(client, CHANNEL, optimistic("hi", "local-1"));
  const row = timeline(client).find((event) => event.id === "local-1");
  assert.equal(row?.content, "hi");
  assert.equal(row?.pending, true);
});

test("a failed send keeps its row, marked failed, and retry clears the mark in place", () => {
  const { client } = harness();
  const preview = optimistic("hi", "local-1");
  writeOptimisticMessage(client, CHANNEL, preview);
  updateOptimisticMessage(client, CHANNEL, "local-1", (event) => ({
    ...event,
    failed: true,
  }));
  let row = timeline(client).find((event) => event.id === "local-1");
  assert.equal(row?.failed, true);
  assert.equal(row?.pending, true);

  // Retry rewrites the same row with the same id and time.
  writeOptimisticMessage(client, CHANNEL, {
    ...optimistic("hi", "local-1"),
    created_at: preview.created_at,
  });
  const rows = timeline(client).filter((event) => event.id === "local-1");
  assert.equal(rows.length, 1);
  row = rows[0];
  assert.notEqual(row.failed, true);
  assert.equal(row.created_at, preview.created_at);
});

test("deleting a failed row removes it from the timeline and its thread", () => {
  const { client, root } = harness();
  client.setQueryData(threadRepliesKey(CHANNEL, root.id), []);
  writeOptimisticMessage(
    client,
    CHANNEL,
    optimistic("reply", "local-2", root.id, [root]),
  );
  removeOptimisticMessage(client, CHANNEL, "local-2");
  assert.equal(
    timeline(client).some((event) => event.id === "local-2"),
    false,
  );
  assert.deepEqual(client.getQueryData(threadRepliesKey(CHANNEL, root.id)), []);
});

test("an own thread reply shows in an open thread at once", () => {
  const { client, root } = harness();
  client.setQueryData(threadRepliesKey(CHANNEL, root.id), []);
  writeOptimisticMessage(
    client,
    CHANNEL,
    optimistic("reply", "local-3", root.id, [root]),
  );
  const replies = client.getQueryData(threadRepliesKey(CHANNEL, root.id));
  assert.deepEqual(
    replies.map((event) => event.id),
    ["local-3"],
  );
});

test("an unopened thread is left unloaded so its first fetch still runs", () => {
  const { client, root } = harness();
  writeOptimisticMessage(
    client,
    CHANNEL,
    optimistic("reply", "local-4", root.id, [root]),
  );
  assert.equal(
    client.getQueryData(threadRepliesKey(CHANNEL, root.id)),
    undefined,
  );
});

test("a relay echo adopts a failed row (send timed out after the relay stored it)", () => {
  const failed = { ...optimistic("hi", "local-5"), failed: true };
  const echo = sent("hi", failed.created_at);
  echo.content = "hi";
  const merged = reconcileIncomingMessage([failed], echo);
  assert.equal(merged.length, 1);
  assert.equal(merged[0].id, echo.id);
  assert.equal(merged[0].localKey, "local-5");
});

test("a newest thread page keeps own unsent replies", () => {
  const pendingReply = {
    ...optimistic("reply", "local-6"),
    tags: [
      ["h", CHANNEL],
      ["e", ROOT, "", "root"],
      ["e", ROOT, "", "reply"],
    ],
    created_at: 1,
    failed: true,
  };
  const pageReply = sent("p", 50, [
    ["e", ROOT, "", "root"],
    ["e", ROOT, "", "reply"],
  ]);
  const { events } = mergeNewestThreadPage([pendingReply], undefined, {
    events: [pageReply],
    hasMore: false,
    nextCursor: null,
  });
  assert.ok(events.some((event) => event.id === "local-6"));
});

test("failed sends run retry or delete once", () => {
  resetFailedSends();
  const calls = [];
  registerFailedSend("k", {
    retry: () => calls.push("retry"),
    discard: () => calls.push("discard"),
  });
  assert.equal(hasFailedSend("k"), true);
  retryFailedSend("k");
  retryFailedSend("k");
  discardFailedSend("k");
  assert.deepEqual(calls, ["retry"]);
  assert.equal(hasFailedSend("k"), false);
});

test("an echo of a send marked failed takes its place in the main timeline", () => {
  const { client } = harness();
  writeOptimisticMessage(client, CHANNEL, optimistic("hi", "local-7"));
  updateOptimisticMessage(client, CHANNEL, "local-7", (event) => ({
    ...event,
    failed: true,
  }));
  const failed = timeline(client).find((event) => event.id === "local-7");
  const echo = { ...sent("echo", failed.created_at), content: "hi" };
  writeOptimisticMessage(client, CHANNEL, echo);
  const rows = timeline(client).filter((event) => event.content === "hi");
  assert.equal(rows.length, 1);
  assert.equal(rows[0].id, echo.id);
  assert.equal(rows[0].localKey, "local-7");
});

test("the composer flow can supply the retry for a failed row", () => {
  resetFailedSends();
  const calls = [];
  registerFailedSend("k", {
    retry: () => calls.push("plain"),
    discard: () => calls.push("discard"),
  });
  replaceFailedSendRetry("k", () => calls.push("flow"));
  replaceFailedSendRetry("missing", () => calls.push("never"));
  retryFailedSend("k");
  assert.deepEqual(calls, ["flow"]);
});
