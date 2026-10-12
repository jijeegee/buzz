import { expect, test, type Page } from "@playwright/test";

import { TEST_IDENTITIES, installMockBridge } from "../helpers/bridge";

/**
 * Newest-first threads: a long thread opens at its newest replies (one
 * `get_thread_windows` page), pages older replies on scroll-up without the
 * view jumping, reopens from cache, and an inbox thread row opens the thread
 * beside the room without moving the room off its newest messages.
 */

const GENERAL_CHANNEL_ID = "9a1657ac-f7aa-5db0-b632-d8bbeb6dfb50";
const REPLY_COUNT = 120;
const FILLER_COUNT = 30;
const ROOT_ID = "nfroot0001";
const INBOX_FILTER_STORAGE_KEY = "buzz.desktop.inbox-filter";
/** Mock latency for get_thread_windows, so in-flight states are observable. */
const THREAD_DELAY_MS = 600;

type RelayEventLike = {
  id: string;
  created_at: number;
  kind: number;
  pubkey: string;
  content: string;
  tags: string[][];
};

type MockWindow = Window & {
  __BUZZ_E2E_EMIT_MOCK_MESSAGE__?: (input: {
    channelName: string;
    content: string;
    parentEventId?: string;
    pubkey?: string;
    createdAt?: number;
    id?: string;
  }) => RelayEventLike;
  __BUZZ_E2E_PUSH_MOCK_FEED_ITEM__?: (item: {
    category: "mention" | "needs_action" | "activity" | "agent_activity";
    channel_id: string | null;
    channel_name: string;
    content: string;
    created_at: number;
    id: string;
    kind: number;
    pubkey: string;
    tags: string[][];
  }) => unknown;
  __BUZZ_E2E_COMMAND_PAYLOADS__?: Array<{ command: string; payload: unknown }>;
};

type ThreadWindowsPayload = {
  threads: Array<{
    channelId: string;
    rootId: string;
    cursor: { created_at: number; event_id: string } | null;
  }>;
  limitRows: number;
};

function replyText(index: number) {
  return `NF reply ${String(index).padStart(3, "0")}`;
}

/**
 * Seed a root, its replies and newer top-level filler into #general's mock
 * store before the channel is opened, so nothing reaches the thread cache
 * through the live subscription: the panel must load via the relay command.
 */
async function seedLongThread(page: Page, fillerCount = FILLER_COUNT) {
  await page.waitForFunction(
    () =>
      typeof (window as MockWindow).__BUZZ_E2E_EMIT_MOCK_MESSAGE__ ===
      "function",
  );
  return page.evaluate(
    ({ rootId, replies, fillers, alice }) => {
      const emit = (window as MockWindow).__BUZZ_E2E_EMIT_MOCK_MESSAGE__;
      if (!emit) throw new Error("emitter missing");
      const now = Math.floor(Date.now() / 1000);
      const base = now - 5_000;
      emit({
        channelName: "general",
        content: "NF thread root",
        createdAt: base,
        id: rootId,
        pubkey: alice,
      });
      let latest: RelayEventLike | null = null;
      for (let i = 0; i < replies; i += 1) {
        latest = emit({
          channelName: "general",
          content: `NF reply ${String(i).padStart(3, "0")}`,
          parentEventId: rootId,
          createdAt: base + 10 + i,
          pubkey: alice,
        });
      }
      for (let i = 0; i < fillers; i += 1) {
        emit({
          channelName: "general",
          content: `NF filler ${i}\nsecond line\nthird line`,
          createdAt: base + 1_000 + i,
          pubkey: alice,
        });
      }
      return latest;
    },
    {
      rootId: ROOT_ID,
      replies: REPLY_COUNT,
      fillers: fillerCount,
      alice: TEST_IDENTITIES.alice.pubkey,
    },
  );
}

async function clearCommandLog(page: Page) {
  await page.evaluate(() => {
    const log = (window as MockWindow).__BUZZ_E2E_COMMAND_PAYLOADS__;
    if (log) log.length = 0;
  });
}

async function threadWindowCalls(page: Page): Promise<ThreadWindowsPayload[]> {
  return page.evaluate(
    () =>
      ((window as MockWindow).__BUZZ_E2E_COMMAND_PAYLOADS__ ?? [])
        .filter((entry) => entry.command === "get_thread_windows")
        .map((entry) => entry.payload) as ThreadWindowsPayload[],
  );
}

async function fullThreadReads(page: Page) {
  return page.evaluate(
    () =>
      ((window as MockWindow).__BUZZ_E2E_COMMAND_PAYLOADS__ ?? []).filter(
        (entry) => entry.command === "get_thread_replies",
      ).length,
  );
}

function threadPanel(page: Page) {
  return page.getByTestId("message-thread-panel");
}

function threadReply(page: Page, index: number) {
  return threadPanel(page).getByText(replyText(index), { exact: true });
}

async function distanceFromBottom(page: Page, testId: string) {
  return page.getByTestId(testId).evaluate((element) => {
    const el = element as HTMLElement;
    return el.scrollHeight - el.scrollTop - el.clientHeight;
  });
}

async function openGeneral(page: Page) {
  await page.getByTestId("channel-general").click();
  await expect(page.getByTestId("chat-title")).toHaveText("general");
}

async function openThreadFromSummary(page: Page) {
  const summary = page.locator(
    `[data-testid="message-thread-summary"][data-thread-head-id="${ROOT_ID}"]`,
  );
  await summary.first().scrollIntoViewIfNeeded();
  await summary.first().click();
  await expect(threadPanel(page)).toBeVisible();
}

/** Offset of a reply's top edge from the thread body's top edge. */
async function replyOffsetByText(page: Page, text: string) {
  return page.getByTestId("message-thread-body").evaluate((body, wanted) => {
    const bodyTop = body.getBoundingClientRect().top;
    const node = Array.from(body.querySelectorAll("*")).find(
      (el) => el.childElementCount === 0 && el.textContent?.trim() === wanted,
    );
    return node ? node.getBoundingClientRect().top - bodyTop : null;
  }, text);
}

/** The topmost reply visible in the thread body, with its offset. */
async function firstVisibleReply(page: Page) {
  return page.getByTestId("message-thread-body").evaluate((body) => {
    const bodyRect = body.getBoundingClientRect();
    const nodes = Array.from(body.querySelectorAll("*")).filter(
      (el) =>
        el.childElementCount === 0 &&
        /^NF reply \d{3}$/.test(el.textContent?.trim() ?? ""),
    );
    for (const node of nodes) {
      const rect = node.getBoundingClientRect();
      if (rect.top >= bodyRect.top && rect.bottom <= bodyRect.bottom) {
        return {
          text: node.textContent?.trim() ?? "",
          top: rect.top - bodyRect.top,
        };
      }
    }
    return null;
  });
}

test.describe("newest-first threads", () => {
  test("opens at newest replies, pages older on scroll-up, reopens from cache", async ({
    page,
  }) => {
    await installMockBridge(page, { threadRepliesDelayMs: THREAD_DELAY_MS });
    await page.goto("/");
    await seedLongThread(page);
    await openGeneral(page);

    // 1. Opening from the timeline loads one newest page and pins to bottom.
    await clearCommandLog(page);
    await openThreadFromSummary(page);
    await expect(threadReply(page, REPLY_COUNT - 1)).toBeVisible();
    await expect
      .poll(() => distanceFromBottom(page, "message-thread-body"))
      .toBeLessThan(4);
    await expect(threadReply(page, 0)).toHaveCount(0);
    await expect(threadReply(page, REPLY_COUNT - 50)).toHaveCount(1);
    await expect(threadReply(page, REPLY_COUNT - 51)).toHaveCount(0);
    const firstCalls = (await threadWindowCalls(page)).filter((call) =>
      call.threads.some((thread) => thread.rootId === ROOT_ID),
    );
    expect(firstCalls).toHaveLength(1);
    expect(firstCalls[0].limitRows).toBe(50);
    expect(firstCalls[0].threads).toEqual([
      { channelId: GENERAL_CHANNEL_ID, rootId: ROOT_ID, cursor: null },
    ]);
    expect(await fullThreadReads(page)).toBe(0);
    await expect(page.getByTestId("message-thread-older-replies")).toHaveCount(
      1,
    );

    // 2. Wheel-scrolling up loads the next older page (a cursor request);
    // the reply at the top of the view keeps its place when the page lands.
    // get_thread_windows answers after THREAD_DELAY_MS, so the view can be
    // measured while the older page is in flight.
    const body = page.getByTestId("message-thread-body");
    await body.hover();
    const hasCursorCall = async () =>
      (await threadWindowCalls(page)).some((call) =>
        call.threads.some(
          (thread) => thread.rootId === ROOT_ID && thread.cursor !== null,
        ),
      );
    await expect
      .poll(
        async () => {
          if (await hasCursorCall()) return true;
          await page.mouse.wheel(0, -400);
          return hasCursorCall();
        },
        { intervals: [120], timeout: 10_000 },
      )
      .toBe(true);
    const anchor = await firstVisibleReply(page);
    expect(anchor).not.toBeNull();
    await expect(threadReply(page, REPLY_COUNT - 51)).toHaveCount(1);
    await page.waitForTimeout(300);
    const olderCall = (await threadWindowCalls(page)).find((call) =>
      call.threads.some((thread) => thread.cursor !== null),
    );
    expect(olderCall?.limitRows).toBe(50);
    expect(olderCall?.threads[0].cursor).toMatchObject({
      created_at: expect.any(Number),
      event_id: expect.any(String),
    });
    const scrollTopAfter = await body.evaluate((el) => el.scrollTop);
    const anchorAfter = await replyOffsetByText(page, anchor?.text ?? "");
    console.log(
      `[scenario 2] anchor "${anchor?.text}" offset before=${anchor?.top} after=${anchorAfter} scrollTop after=${scrollTopAfter}`,
    );
    expect(scrollTopAfter).toBeGreaterThan(0);
    expect(anchorAfter).not.toBeNull();
    expect(Math.abs((anchorAfter ?? 0) - (anchor?.top ?? 0))).toBeLessThan(40);

    // Keep wheeling up until the oldest reply arrives.
    await expect
      .poll(
        async () => {
          await page.mouse.wheel(0, -1500);
          return threadReply(page, 0).count();
        },
        { intervals: [250], timeout: 20_000 },
      )
      .toBe(1);
    await expect(page.getByTestId("message-thread-older-replies")).toHaveCount(
      0,
    );

    // 3. Close and reopen: replies show at once and only the newest page is
    // revalidated (never a refetch from the oldest reply).
    await page.getByTestId("auxiliary-panel-close").click();
    await expect(threadPanel(page)).not.toBeVisible();
    await clearCommandLog(page);
    await openThreadFromSummary(page);
    await expect(page.getByTestId("message-thread-replies-loading")).toHaveCount(
      0,
    );
    await expect(threadReply(page, REPLY_COUNT - 1)).toBeVisible();
    await page.waitForTimeout(500);
    const reopenCalls = await threadWindowCalls(page);
    console.log(
      `[scenario 3] reopen get_thread_windows calls=${JSON.stringify(reopenCalls)}`,
    );
    for (const call of reopenCalls) {
      for (const thread of call.threads) expect(thread.cursor).toBeNull();
    }
    expect(await fullThreadReads(page)).toBe(0);
  });

  test("thread deep link opens at newest reply, room stays at bottom; Show in channel finds root", async ({
    page,
  }) => {
    await installMockBridge(page);
    await page.goto("/");
    await seedLongThread(page);

    await page.goto(`/#/channels/${GENERAL_CHANNEL_ID}?thread=${ROOT_ID}`);
    await expect(page.getByTestId("chat-title")).toHaveText("general");
    await expect(threadPanel(page)).toBeVisible();
    await expect(threadReply(page, REPLY_COUNT - 1)).toBeVisible();
    await expect
      .poll(() => distanceFromBottom(page, "message-thread-body"))
      .toBeLessThan(4);
    await expect(page.getByText(`NF filler ${FILLER_COUNT - 1}`)).toBeVisible();
    await expect
      .poll(() => distanceFromBottom(page, "message-timeline"))
      .toBeLessThan(40);

    // 5. Show in channel brings the root row into the room's viewport.
    const rootRow = page.locator(
      `[data-testid="message-timeline"] [data-message-id="${ROOT_ID}"]`,
    );
    await page.getByTestId("message-thread-show-in-channel").click();
    await expect(rootRow.first()).toBeInViewport();
  });

  for (const fillers of [FILLER_COUNT, 5]) {
    test(`inbox thread row opens the thread at its newest reply without moving the room (${fillers} newer room messages)`, async ({
      page,
    }) => {
      await page.addInitScript((key) => {
        window.localStorage.setItem(key, "conversations");
      }, INBOX_FILTER_STORAGE_KEY);
      await installMockBridge(page);
      await page.goto("/");
      const latest = await seedLongThread(page, fillers);
      if (!latest) throw new Error("no reply seeded");
      await page.evaluate(
        ({ event, channelId }) => {
          (window as MockWindow).__BUZZ_E2E_PUSH_MOCK_FEED_ITEM__?.({
            category: "mention",
            channel_id: channelId,
            channel_name: "general",
            content: event.content,
            created_at: event.created_at,
            id: event.id,
            kind: event.kind,
            pubkey: event.pubkey,
            tags: event.tags,
          });
        },
        { event: latest, channelId: GENERAL_CHANNEL_ID },
      );

      const row = page.getByTestId(`home-inbox-item-${latest.id}`);
      await expect(row).toBeVisible({ timeout: 15_000 });
      await row.click();

      await expect(page.getByTestId("chat-title")).toHaveText("general");
      await expect(page).toHaveURL(new RegExp(`thread=${ROOT_ID}`));
      expect(page.url()).not.toContain("messageId");
      await expect(threadPanel(page)).toBeVisible();
      await expect(threadReply(page, REPLY_COUNT - 1)).toBeVisible();
      await expect
        .poll(() => distanceFromBottom(page, "message-thread-body"))
        .toBeLessThan(4);
      await expect(page.getByText(`NF filler ${fillers - 1}`)).toBeVisible();
      await expect
        .poll(() => distanceFromBottom(page, "message-timeline"))
        .toBeLessThan(40);
    });
  }
});
