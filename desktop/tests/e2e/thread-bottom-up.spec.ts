import { expect, test, type Page } from "@playwright/test";

import { TEST_IDENTITIES, installMockBridge } from "../helpers/bridge";

/**
 * Bottom-anchored thread panel: a short thread stacks just above the composer,
 * a long thread opens on its newest reply in every painted frame, and the
 * thread head is the top end of the list (painted only once every older reply
 * is loaded, never painted first and then removed).
 */

const LONG_ROOT_ID = "burootlong1";
const SHORT_ROOT_ID = "burootshort";
const LONG_REPLY_COUNT = 120;
const SHORT_REPLY_COUNT = 3;
/** Mock latency for get_thread_windows, so the loading phase is sampled. */
const THREAD_DELAY_MS = 600;

type MockWindow = Window & {
  __KUZZ_E2E_EMIT_MOCK_MESSAGE__?: (input: {
    channelName: string;
    content: string;
    parentEventId?: string;
    pubkey?: string;
    createdAt?: number;
    id?: string;
  }) => unknown;
  __BU_FRAMES__?: FrameSample[];
  __BU_STOP__?: boolean;
};

type FrameSample = {
  headCount: number;
  olderRowCount: number;
  replyCount: number;
  distanceFromBottom: number | null;
};

function longReplyText(index: number) {
  return `BU long ${String(index).padStart(3, "0")}`;
}

function shortReplyText(index: number) {
  return `BU short ${index}`;
}

async function seedThreads(page: Page) {
  await page.waitForFunction(
    () =>
      typeof (window as MockWindow).__KUZZ_E2E_EMIT_MOCK_MESSAGE__ ===
      "function",
  );
  await page.evaluate(
    ({ longRoot, shortRoot, longCount, shortCount, alice }) => {
      const emit = (window as MockWindow).__KUZZ_E2E_EMIT_MOCK_MESSAGE__;
      if (!emit) throw new Error("emitter missing");
      const base = Math.floor(Date.now() / 1000) - 5_000;
      emit({
        channelName: "general",
        content: "BU long root",
        createdAt: base,
        id: longRoot,
        pubkey: alice,
      });
      for (let i = 0; i < longCount; i += 1) {
        emit({
          channelName: "general",
          content: `BU long ${String(i).padStart(3, "0")}`,
          parentEventId: longRoot,
          createdAt: base + 10 + i,
          pubkey: alice,
        });
      }
      emit({
        channelName: "general",
        content: "BU short root",
        createdAt: base + 1_000,
        id: shortRoot,
        pubkey: alice,
      });
      for (let i = 0; i < shortCount; i += 1) {
        emit({
          channelName: "general",
          content: `BU short ${i}`,
          parentEventId: shortRoot,
          createdAt: base + 1_010 + i,
          pubkey: alice,
        });
      }
    },
    {
      longRoot: LONG_ROOT_ID,
      shortRoot: SHORT_ROOT_ID,
      longCount: LONG_REPLY_COUNT,
      shortCount: SHORT_REPLY_COUNT,
      alice: TEST_IDENTITIES.alice.pubkey,
    },
  );
}

function threadPanel(page: Page) {
  return page.getByTestId("message-thread-panel");
}

async function openGeneral(page: Page) {
  await page.getByTestId("channel-general").click();
  await expect(page.getByTestId("chat-title")).toHaveText("general");
}

async function openThread(page: Page, rootId: string) {
  const summary = page.locator(
    `[data-testid="message-thread-summary"][data-thread-head-id="${rootId}"]`,
  );
  await summary.first().scrollIntoViewIfNeeded();
  await summary.first().click();
  await expect(threadPanel(page)).toBeVisible();
}

/** Samples the thread panel on every animation frame until stopped. */
async function startFrameSampler(page: Page) {
  await page.evaluate(() => {
    const w = window as MockWindow;
    w.__BU_FRAMES__ = [];
    w.__BU_STOP__ = false;
    const tick = () => {
      if (w.__BU_STOP__) return;
      const body = document.querySelector<HTMLElement>(
        '[data-testid="message-thread-body"]',
      );
      if (body) {
        const leaves = Array.from(body.querySelectorAll("*")).filter(
          (el) =>
            el.childElementCount === 0 &&
            /^BU long \d{3}$/.test(el.textContent?.trim() ?? ""),
        );
        w.__BU_FRAMES__?.push({
          headCount: body.querySelectorAll(
            '[data-testid="message-thread-head"]',
          ).length,
          olderRowCount: body.querySelectorAll(
            '[data-testid="message-thread-older-replies"]',
          ).length,
          replyCount: leaves.length,
          distanceFromBottom:
            body.scrollHeight - body.scrollTop - body.clientHeight,
        });
      }
      requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  });
}

async function stopFrameSampler(page: Page): Promise<FrameSample[]> {
  return page.evaluate(() => {
    const w = window as MockWindow;
    w.__BU_STOP__ = true;
    return w.__BU_FRAMES__ ?? [];
  });
}

test.describe("bottom-anchored thread panel", () => {
  test("a short thread stacks just above the composer", async ({ page }) => {
    await installMockBridge(page);
    await page.goto("/");
    await seedThreads(page);
    await openGeneral(page);
    await openThread(page, SHORT_ROOT_ID);

    const panel = threadPanel(page);
    const lastReply = panel.getByText(shortReplyText(SHORT_REPLY_COUNT - 1), {
      exact: true,
    });
    await expect(lastReply).toBeVisible();
    // Nothing older to load, so the head is drawn at the top of the stack.
    await expect(panel.getByTestId("message-thread-head")).toContainText(
      "BU short root",
    );
    await expect(panel.getByTestId("message-thread-older-replies")).toHaveCount(
      0,
    );

    const geometry = await page.evaluate(
      ({ lastText }) => {
        const body = document.querySelector<HTMLElement>(
          '[data-testid="message-thread-body"]',
        );
        const head = document.querySelector<HTMLElement>(
          '[data-testid="message-thread-head"]',
        );
        const composer = document.querySelector<HTMLElement>(
          '[data-testid="thread-composer-overlay"]',
        );
        const last = Array.from(body?.querySelectorAll("*") ?? []).find(
          (el) =>
            el.childElementCount === 0 && el.textContent?.trim() === lastText,
        );
        if (!body || !head || !composer || !last) return null;
        const bodyRect = body.getBoundingClientRect();
        return {
          overflow: body.scrollHeight - body.clientHeight,
          gapAboveHead: head.getBoundingClientRect().top - bodyRect.top,
          gapAboveComposer:
            composer.getBoundingClientRect().top -
            last.getBoundingClientRect().bottom,
        };
      },
      { lastText: shortReplyText(SHORT_REPLY_COUNT - 1) },
    );
    console.log(`[short thread] ${JSON.stringify(geometry)}`);
    expect(geometry).not.toBeNull();
    // The short thread does not fill the panel: empty space sits above it...
    expect(geometry?.overflow ?? 1).toBeLessThanOrEqual(1);
    expect(geometry?.gapAboveHead ?? 0).toBeGreaterThan(150);
    // ...and the newest reply sits right above the composer, not under it.
    expect(geometry?.gapAboveComposer ?? -1).toBeGreaterThanOrEqual(0);
    expect(geometry?.gapAboveComposer ?? 999).toBeLessThan(120);
  });

  test("a long thread opens on its newest reply in every frame and shows its head only at the top", async ({
    page,
  }) => {
    await installMockBridge(page, { threadRepliesDelayMs: THREAD_DELAY_MS });
    await page.goto("/");
    await seedThreads(page);
    await openGeneral(page);

    await startFrameSampler(page);
    await openThread(page, LONG_ROOT_ID);
    const panel = threadPanel(page);
    await expect(
      panel.getByText(longReplyText(LONG_REPLY_COUNT - 1), { exact: true }),
    ).toBeVisible();
    await page.waitForTimeout(400);
    const frames = await stopFrameSampler(page);

    const loadingFrames = frames.filter((frame) => frame.replyCount === 0);
    const replyFrames = frames.filter((frame) => frame.replyCount > 0);
    console.log(
      `[long thread open] frames=${frames.length} loading=${loadingFrames.length} withReplies=${replyFrames.length} maxDistance=${Math.max(
        ...replyFrames.map((frame) => frame.distanceFromBottom ?? 0),
      )}`,
    );
    expect(loadingFrames.length).toBeGreaterThan(0);
    expect(replyFrames.length).toBeGreaterThan(0);
    // The head is never painted while replies load or older replies remain:
    // there is no frame that draws it first and removes it later.
    expect(frames.every((frame) => frame.headCount === 0)).toBe(true);
    // From the first frame that shows any reply, the view is at the bottom.
    for (const frame of replyFrames) {
      expect(frame.distanceFromBottom ?? 999).toBeLessThan(4);
    }
    // The older-replies row sits at the top end of the list, above the replies.
    expect(replyFrames.every((frame) => frame.olderRowCount === 1)).toBe(true);

    // Scrolling up to the oldest reply puts the head at the top of the list.
    const body = page.getByTestId("message-thread-body");
    await body.hover();
    await expect
      .poll(
        async () => {
          await page.mouse.wheel(0, -1500);
          return panel.getByText(longReplyText(0), { exact: true }).count();
        },
        { intervals: [250], timeout: 20_000 },
      )
      .toBe(1);
    await expect(panel.getByTestId("message-thread-older-replies")).toHaveCount(
      0,
    );
    const head = panel.getByTestId("message-thread-head");
    await expect(head).toContainText("BU long root");
    const order = await page.evaluate(() => {
      const body = document.querySelector<HTMLElement>(
        '[data-testid="message-thread-body"]',
      );
      const head = body?.querySelector<HTMLElement>(
        '[data-testid="message-thread-head"]',
      );
      const first = Array.from(body?.querySelectorAll("*") ?? []).find(
        (el) =>
          el.childElementCount === 0 &&
          el.textContent?.trim() === "BU long 000",
      );
      if (!head || !first) return null;
      return (
        head.getBoundingClientRect().bottom <= first.getBoundingClientRect().top
      );
    });
    expect(order).toBe(true);
  });
});
