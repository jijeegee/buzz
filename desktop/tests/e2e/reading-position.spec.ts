import { expect, test } from "@playwright/test";

import { TEST_IDENTITIES, installMockBridge } from "../helpers/bridge";

// Emits `count` already-read rows to general: dated well before the read
// frontier captured when general was last open.
async function emitReadMessages(
  page: import("@playwright/test").Page,
  count: number,
) {
  await page.evaluate(
    ({ total, base, pubkey }) => {
      const emit = (
        window as Window & {
          __KUZZ_E2E_EMIT_MOCK_MESSAGE__?: (input: {
            channelName: string;
            content: string;
            pubkey: string;
            createdAt?: number;
          }) => unknown;
        }
      ).__KUZZ_E2E_EMIT_MOCK_MESSAGE__;
      for (let index = 0; index < total; index += 1) {
        emit?.({
          channelName: "general",
          content: `Read message ${index + 1}`,
          pubkey,
          createdAt: base + index,
        });
      }
    },
    {
      total: count,
      base: Math.floor(Date.now() / 1000) - 3_600,
      pubkey: TEST_IDENTITIES.alice.pubkey,
    },
  );
}

async function openChannel(
  page: import("@playwright/test").Page,
  name: string,
) {
  await page.getByTestId(`channel-${name}`).click();
  await expect(page.getByTestId("chat-title")).toHaveText(name);
}

type RowPosition = { messageId: string; top: number };

// The first message row whose top is at or below the top chrome.
async function readTopRow(
  page: import("@playwright/test").Page,
): Promise<RowPosition> {
  return page.evaluate(() => {
    const timeline = document.querySelector<HTMLElement>(
      '[data-testid="message-timeline"]',
    );
    if (!timeline) throw new Error("missing timeline");
    const timelineTop = timeline.getBoundingClientRect().top;
    const chrome = Number.parseFloat(getComputedStyle(timeline).paddingTop);
    const rows = [
      ...timeline.querySelectorAll<HTMLElement>("[data-message-id]"),
    ].map((row) => ({
      messageId: row.dataset.messageId ?? "",
      top: row.getBoundingClientRect().top - timelineTop,
    }));
    const row = rows
      .filter((candidate) => candidate.top >= chrome)
      .sort((a, b) => a.top - b.top)[0];
    if (!row) throw new Error("no row under the top chrome");
    return row;
  });
}

// Samples each frame from now on: the top of `messageId`'s row while the
// general timeline is visibly painted, or null when it is not painted.
async function startRowSampler(
  page: import("@playwright/test").Page,
  messageId: string,
) {
  await page.evaluate((id) => {
    const frames: (number | null)[] = [];
    const sampler = window as Window & {
      __rowFrames?: (number | null)[];
      __rowStop?: boolean;
    };
    sampler.__rowFrames = frames;
    sampler.__rowStop = false;
    const step = () => {
      if (sampler.__rowStop) return;
      const timeline = document.querySelector<HTMLElement>(
        '[data-testid="message-timeline"]',
      );
      const title = document.querySelector('[data-testid="chat-title"]');
      const visible =
        timeline !== null &&
        title?.textContent === "general" &&
        getComputedStyle(timeline).visibility !== "hidden" &&
        timeline.querySelector("[data-message-id]") !== null;
      const row = visible
        ? timeline?.querySelector<HTMLElement>(`[data-message-id="${id}"]`)
        : null;
      frames.push(
        !visible
          ? null
          : row && timeline
            ? row.getBoundingClientRect().top -
              timeline.getBoundingClientRect().top
            : Number.NaN,
      );
      requestAnimationFrame(step);
    };
    requestAnimationFrame(step);
  }, messageId);
}

async function stopRowSampler(page: import("@playwright/test").Page) {
  return page.evaluate(() => {
    const sampler = window as Window & {
      __rowFrames?: (number | null)[];
      __rowStop?: boolean;
    };
    sampler.__rowStop = true;
    return sampler.__rowFrames ?? [];
  });
}

async function scrollUpAndRest(page: import("@playwright/test").Page) {
  await page.getByTestId("message-timeline").hover();
  for (let step = 0; step < 6; step += 1) {
    await page.mouse.wheel(0, -400);
    await page.waitForTimeout(60);
  }
  // Let Virtua report the end of the scroll.
  await page.waitForTimeout(600);
}

test.describe("reading position", () => {
  test("01-reopens-where-the-reader-left", async ({ page }) => {
    await installMockBridge(page);
    await page.goto("/");
    await openChannel(page, "general");
    await openChannel(page, "random");
    await emitReadMessages(page, 120);

    await openChannel(page, "general");
    await expect(
      page.getByText("Read message 120", { exact: true }),
    ).toBeVisible();
    await scrollUpAndRest(page);
    const left = await readTopRow(page);

    await openChannel(page, "random");
    await startRowSampler(page, left.messageId);
    await openChannel(page, "general");
    await expect(
      page.locator(`[data-message-id="${left.messageId}"]`),
    ).toBeVisible();
    await page.waitForTimeout(500);
    const frames = await stopRowSampler(page);
    const shown = frames.filter((frame) => frame !== null);
    expect(shown.length).toBeGreaterThan(5);
    // Every painted frame already has the row where the reader left it.
    for (const top of shown) {
      expect(Math.abs(top - left.top)).toBeLessThanOrEqual(2);
    }
    await expect(
      page.getByText("Read message 120", { exact: true }),
    ).not.toBeInViewport();
  });

  test("02-left-at-newest-reopens-at-newest", async ({ page }) => {
    await installMockBridge(page);
    await page.goto("/");
    await openChannel(page, "general");
    await openChannel(page, "random");
    await emitReadMessages(page, 120);

    await openChannel(page, "general");
    await scrollUpAndRest(page);
    // Back down to the newest message before leaving.
    await page.getByTestId("message-timeline").hover();
    for (let step = 0; step < 12; step += 1) {
      await page.mouse.wheel(0, 800);
      await page.waitForTimeout(60);
    }
    await page.waitForTimeout(600);

    await openChannel(page, "random");
    await openChannel(page, "general");
    await expect(
      page.getByText("Read message 120", { exact: true }),
    ).toBeInViewport();
  });

  test("03-unread-wins-over-remembered-position", async ({ page }) => {
    await installMockBridge(page);
    await page.goto("/");
    await openChannel(page, "general");
    await openChannel(page, "random");
    await emitReadMessages(page, 120);

    await openChannel(page, "general");
    await scrollUpAndRest(page);
    await openChannel(page, "random");
    await page.evaluate((pubkey) => {
      (
        window as Window & {
          __KUZZ_E2E_EMIT_MOCK_MESSAGE__?: (input: {
            channelName: string;
            content: string;
            pubkey: string;
            createdAt?: number;
          }) => unknown;
        }
      ).__KUZZ_E2E_EMIT_MOCK_MESSAGE__?.({
        channelName: "general",
        content: "Fresh unread",
        pubkey,
        createdAt: Math.floor(Date.now() / 1000) + 60,
      });
    }, TEST_IDENTITIES.alice.pubkey);

    await openChannel(page, "general");
    await expect(page.getByTestId("message-unread-divider")).toBeInViewport();
    await expect(
      page.getByText("Fresh unread", { exact: true }),
    ).toBeInViewport();
  });
});
