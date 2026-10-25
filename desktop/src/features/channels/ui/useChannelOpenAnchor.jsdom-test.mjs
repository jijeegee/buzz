import assert from "node:assert/strict";
import { afterEach, test } from "node:test";
import { act, cleanup, renderHook } from "@testing-library/react";

import {
  MAX_UNREAD_SEEK_PAGES,
  SEEK_RETRY_MS,
  useChannelOpenAnchor,
} from "./useChannelOpenAnchor.ts";

// Where a room opens while its first unread is older than the loaded rows:
// how the seek counts pages, retries and gives up.

const CHANNEL = "9a1657ac-f7aa-5db0-b632-d8bbeb6dfb50";

function message(n) {
  return {
    content: `m${n}`,
    created_at: n,
    id: n.toString(16).padStart(64, "0"),
    kind: 9,
    pubkey: "b".repeat(64),
    sig: "",
    tags: [["h", CHANNEL]],
  };
}

/** Every loaded row is unread (newer than the frontier at 10). */
const unreadRows = [message(100), message(101)];

/**
 * Renders the hook over rows that are all unread, answering each older page
 * with the next scripted outcome ("store" once the script runs out).
 */
function render(outcomes, { offline = false } = {}) {
  const asked = [];
  const fetchOlderPage = async () => {
    const outcome = outcomes[asked.length] ?? "store";
    asked.push(outcome);
    return outcome;
  };
  const view = renderHook(() =>
    useChannelOpenAnchor({
      channelId: CHANNEL,
      pubkey: undefined,
      messages: unreadRows,
      frontierSeconds: 10,
      firstUnreadMessageId: unreadRows[0].id,
      hasOlderMessages: true,
      isFetchingOlder: false,
      fetchOlderPage,
      isOffline: () => offline,
      opensElsewhere: false,
    }),
  );
  return { asked, view };
}

async function settle() {
  for (let index = 0; index < 20; index += 1) {
    await act(async () => {
      await Promise.resolve();
    });
  }
}

afterEach(() => cleanup());

test("stored pages do not count toward the server page cap", async () => {
  // Ten stored pages, then a page held by another older load.
  const { asked, view } = render([...Array(10).fill("store"), "busy"]);
  await settle();
  // Well past the server cap, still looking behind the loading state.
  assert.equal(asked.length, 11);
  assert.equal(view.result.current.isResolving, true);
});

test("server pages stop the seek at the cap and open at the newest row", async () => {
  const { asked, view } = render(Array(10).fill("server"));
  await settle();
  assert.equal(asked.length, MAX_UNREAD_SEEK_PAGES);
  assert.equal(view.result.current.isResolving, false);
  assert.equal(view.result.current.anchorMessageId, null);
});

test("a failed page online keeps loading and retries after 8 s", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const { asked, view } = render(["failed", "failed"]);
  await settle();
  assert.equal(asked.length, 1);
  assert.equal(view.result.current.isResolving, true);
  await act(async () => t.mock.timers.tick(SEEK_RETRY_MS - 1));
  await settle();
  assert.equal(asked.length, 1);
  await act(async () => t.mock.timers.tick(1));
  await settle();
  assert.equal(asked.length, 2);
  assert.equal(view.result.current.isResolving, true);
});

test("a failed page offline opens at the newest stored row", async () => {
  const { asked, view } = render(["failed"], { offline: true });
  await settle();
  assert.equal(asked.length, 1);
  assert.equal(view.result.current.isResolving, false);
  assert.equal(view.result.current.anchorMessageId, null);
});

test("a page that appends nothing stops the seek instead of asking again", async () => {
  const { asked, view } = render(["none"]);
  await settle();
  assert.equal(asked.length, 1);
  assert.equal(view.result.current.isResolving, false);
});

test("leaving during a failed seek does not schedule a retry after unmount", async (t) => {
  let finish;
  const pending = new Promise((resolve) => {
    finish = resolve;
  });
  const scheduled = t.mock.method(globalThis, "setTimeout");
  const view = renderHook(() =>
    useChannelOpenAnchor({
      channelId: CHANNEL,
      pubkey: undefined,
      messages: unreadRows,
      frontierSeconds: 10,
      firstUnreadMessageId: unreadRows[0].id,
      hasOlderMessages: true,
      isFetchingOlder: false,
      fetchOlderPage: () => pending,
      isOffline: () => false,
      opensElsewhere: false,
    }),
  );
  view.unmount();
  await act(async () => finish("failed"));
  assert.equal(
    scheduled.mock.calls.filter(
      ({ arguments: args }) => args[1] === SEEK_RETRY_MS,
    ).length,
    0,
  );
});
