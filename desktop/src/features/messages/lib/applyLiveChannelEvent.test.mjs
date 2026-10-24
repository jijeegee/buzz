import assert from "node:assert/strict";
import test from "node:test";
import { QueryClient } from "@tanstack/react-query";

import { reconcileFetchedChannelWindow } from "../hooks.ts";
import { applyLiveChannelEvent } from "./applyLiveChannelEvent.ts";
import { channelNeedsRefresh } from "./channelRoomSyncState.ts";
import {
  CLOSED_CHANNEL_OVERLAY_CAP,
  emptyChannelWindowStore,
  flattenChannelWindowEvents,
  replaceNewestChannelWindow,
} from "./channelWindowStore.ts";
import { reconcileChannelWindowMessages } from "./channelWindowReconciliation.ts";
import {
  channelMessagesKey,
  channelWindowKey,
  threadRepliesKey,
} from "./messageQueryKeys.ts";

const channelId = "room";
const closed = { isOpen: false };
const open = { isOpen: true };

function event(id, createdAt, overrides = {}) {
  return {
    id: id.padEnd(64, "0"),
    pubkey: "a".repeat(64),
    created_at: createdAt,
    kind: 9,
    tags: [["h", channelId]],
    content: id,
    sig: "b".repeat(128),
    ...overrides,
  };
}

function reply(id, createdAt, rootId) {
  return event(id, createdAt, {
    tags: [
      ["h", channelId],
      ["e", rootId, "", "root"],
      ["e", rootId, "", "reply"],
    ],
  });
}

function deletion(id, createdAt, targetId) {
  return event(id, createdAt, {
    kind: 5,
    tags: [
      ["h", channelId],
      ["e", targetId],
    ],
  });
}

function newestPage(rows) {
  return {
    startCursor: null,
    rows: rows.map((row) => ({ event: row, thread: null })),
    aux: [],
    nextCursor: null,
    hasMore: false,
  };
}

function wirePage(rows) {
  return [
    ...rows,
    {
      ...event("bounds", 0),
      kind: 39006,
      tags: [["d", `${channelId}:head`]],
      content: JSON.stringify({ has_more: false, next_cursor: null }),
    },
  ];
}

function client() {
  return new QueryClient({ defaultOptions: { queries: { retry: false } } });
}

/** A room opened earlier and closed since: a relay page and its timeline. */
function cachedRoom(queryClient, rows = [event("history", 100)]) {
  const window = replaceNewestChannelWindow(
    emptyChannelWindowStore(),
    newestPage(rows),
  );
  queryClient.setQueryData(channelWindowKey(channelId), window);
  queryClient.setQueryData(
    channelMessagesKey(channelId),
    reconcileChannelWindowMessages(window, []),
  );
}

const ids = (events) => events.map((candidate) => candidate.id);

test("a closed room's live row survives the next window projection", () => {
  const queryClient = client();
  cachedRoom(queryClient);
  const live = event("live", 200);

  applyLiveChannelEvent(queryClient, channelId, live, closed);

  const window = queryClient.getQueryData(channelWindowKey(channelId));
  assert.deepEqual(ids(window.liveOverlay), [live.id]);
  const shown = queryClient.getQueryData(channelMessagesKey(channelId));
  assert.ok(ids(shown).includes(live.id));
  // The bug: a row written only to the rendered timeline was dropped here.
  const reprojected = reconcileChannelWindowMessages(window, shown);
  assert.ok(ids(reprojected).includes(live.id));
});

test("a never-opened room keeps live rows in its window store only", () => {
  const queryClient = client();
  const live = event("live", 200);

  applyLiveChannelEvent(queryClient, channelId, live, closed);

  assert.equal(
    queryClient.getQueryData(channelMessagesKey(channelId)),
    undefined,
  );
  const window = queryClient.getQueryData(channelWindowKey(channelId));
  assert.deepEqual(window.pages, []);
  assert.deepEqual(ids(window.liveOverlay), [live.id]);

  // Opening fetches history; the live row joins it rather than replacing it.
  const shown = reconcileFetchedChannelWindow(
    queryClient,
    channelId,
    wirePage([event("history", 100)]),
    [],
    new AbortController().signal,
    false,
  );
  assert.deepEqual(ids(shown), [event("history", 100).id, live.id]);
});

test("an overflowing closed room keeps its newest rows and drops its pages", () => {
  const queryClient = client();
  cachedRoom(queryClient);
  const rows = Array.from({ length: CLOSED_CHANNEL_OVERLAY_CAP + 1 }, (_, i) =>
    event(`live-${String(i).padStart(2, "0")}`, 200 + i),
  );

  for (const row of rows.slice(0, CLOSED_CHANNEL_OVERLAY_CAP)) {
    applyLiveChannelEvent(queryClient, channelId, row, closed);
  }
  assert.equal(channelNeedsRefresh(queryClient, channelId), false);
  assert.equal(
    queryClient.getQueryData(channelWindowKey(channelId)).pages.length,
    1,
  );

  applyLiveChannelEvent(queryClient, channelId, rows.at(-1), closed);

  const window = queryClient.getQueryData(channelWindowKey(channelId));
  assert.deepEqual(window.pages, []);
  assert.equal(window.liveOverlay.length, CLOSED_CHANNEL_OVERLAY_CAP);
  assert.ok(!ids(window.liveOverlay).includes(rows[0].id));
  assert.ok(ids(window.liveOverlay).includes(rows.at(-1).id));
  // No timeline is left that would show the page and the kept rows with the
  // trimmed rows silently missing between them.
  assert.equal(
    queryClient.getQueryData(channelMessagesKey(channelId)),
    undefined,
  );
  assert.equal(channelNeedsRefresh(queryClient, channelId), true);

  // The next authoritative newest window clears the mark.
  reconcileFetchedChannelWindow(
    queryClient,
    channelId,
    wirePage(rows.slice(-3).reverse()),
    [],
    new AbortController().signal,
    false,
  );
  assert.equal(channelNeedsRefresh(queryClient, channelId), false);
});

test("an open room's live window is not bounded", () => {
  const queryClient = client();
  cachedRoom(queryClient);
  for (let i = 0; i <= CLOSED_CHANNEL_OVERLAY_CAP; i += 1) {
    applyLiveChannelEvent(
      queryClient,
      channelId,
      event(`l${String(i).padStart(2, "0")}x`, 200 + i),
      open,
    );
  }
  const window = queryClient.getQueryData(channelWindowKey(channelId));
  assert.equal(window.pages.length, 1);
  assert.equal(window.liveOverlay.length, CLOSED_CHANNEL_OVERLAY_CAP + 1);
  assert.equal(
    queryClient.getQueryData(channelMessagesKey(channelId)).length,
    CLOSED_CHANNEL_OVERLAY_CAP + 2,
  );
});

test("a deletion of an overlay-only row is kept and reaches the timeline", () => {
  const queryClient = client();
  const live = event("live", 200);
  const removed = deletion("delete", 201, live.id);

  applyLiveChannelEvent(queryClient, channelId, live, closed);
  applyLiveChannelEvent(queryClient, channelId, removed, closed);

  const window = queryClient.getQueryData(channelWindowKey(channelId));
  assert.deepEqual(ids(window.liveAux), [removed.id]);
  assert.ok(ids(flattenChannelWindowEvents(window)).includes(removed.id));

  // Overflow keeps the deletion while its target row is kept.
  for (let i = 0; i < CLOSED_CHANNEL_OVERLAY_CAP; i += 1) {
    applyLiveChannelEvent(
      queryClient,
      channelId,
      event(`n${String(i).padStart(2, "0")}x`, 300 + i),
      closed,
    );
  }
  const bounded = queryClient.getQueryData(channelWindowKey(channelId));
  assert.ok(!ids(bounded.liveOverlay).includes(live.id));
  assert.deepEqual(bounded.liveAux, []);

  // On open the deletion of a kept row is in the timeline the renderer reads.
  const kept = bounded.liveOverlay[0];
  const keptDeletion = deletion("delete-kept", 400, kept.id);
  applyLiveChannelEvent(queryClient, channelId, keptDeletion, closed);
  const shown = reconcileFetchedChannelWindow(
    queryClient,
    channelId,
    wirePage([]),
    [],
    new AbortController().signal,
    false,
  );
  assert.ok(ids(shown).includes(keptDeletion.id));
});

test("a closed room never creates an unloaded thread cache", () => {
  const queryClient = client();
  cachedRoom(queryClient);
  const rootId = event("history", 100).id;
  const first = reply("reply-1", 200, rootId);

  applyLiveChannelEvent(queryClient, channelId, first, closed);
  assert.equal(
    queryClient.getQueryData(threadRepliesKey(channelId, rootId)),
    undefined,
  );
  // The rendered timeline retains it beside the window, as before.
  assert.ok(
    ids(queryClient.getQueryData(channelMessagesKey(channelId))).includes(
      first.id,
    ),
  );

  queryClient.setQueryData(threadRepliesKey(channelId, rootId), []);
  const second = reply("reply-2", 201, rootId);
  applyLiveChannelEvent(queryClient, channelId, second, closed);
  assert.deepEqual(
    ids(queryClient.getQueryData(threadRepliesKey(channelId, rootId))),
    [second.id],
  );
});

test("a never-opened room's thread reply touches no cache", () => {
  const queryClient = client();
  const rootId = "root".padEnd(64, "0");
  applyLiveChannelEvent(
    queryClient,
    channelId,
    reply("reply", 200, rootId),
    closed,
  );
  assert.equal(
    queryClient.getQueryData(threadRepliesKey(channelId, rootId)),
    undefined,
  );
  assert.equal(
    queryClient.getQueryData(channelMessagesKey(channelId)),
    undefined,
  );
  assert.equal(
    queryClient.getQueryData(channelWindowKey(channelId)),
    undefined,
  );
});
