import { expect, test } from "@playwright/test";

import { TEST_IDENTITIES, installMockBridge } from "../helpers/bridge";

async function waitForMockLiveSubscription(
  page: import("@playwright/test").Page,
  channelName: string,
) {
  await expect
    .poll(async () => {
      return page.evaluate(
        ({ ch }) =>
          (
            window as Window & {
              __KUZZ_E2E_HAS_MOCK_LIVE_SUBSCRIPTION__?: (input: {
                channelName: string;
              }) => boolean;
            }
          ).__KUZZ_E2E_HAS_MOCK_LIVE_SUBSCRIPTION__?.({ channelName: ch }) ??
          false,
        { ch: channelName },
      );
    })
    .toBe(true);
}

function emitMockMessage(
  page: import("@playwright/test").Page,
  channelName: string,
  content: string,
  createdAt?: number,
) {
  return page.evaluate(
    ({ ch, msg, pubkey, ts }) => {
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
        channelName: ch,
        content: msg,
        pubkey,
        createdAt: ts,
      });
    },
    {
      ch: channelName,
      msg: content,
      pubkey: TEST_IDENTITIES.alice.pubkey,
      ts: createdAt,
    },
  );
}

// Unread messages must be created strictly after the read frontier captured
// when the channel was last open. The frontier is captured at the current
// second on open, and computeChannelUnreadMarker uses a strict
// `createdAt > frontier` predicate — so emitting at the same wall-clock second
// leaves the messages on the read side and the pill/divider never render.
// Dating them a minute ahead puts them deterministically past the frontier.
const UNREAD_OFFSET_SECONDS = 60;

function unreadTimestamp() {
  return Math.floor(Date.now() / 1000) + UNREAD_OFFSET_SECONDS;
}

// Emit `count` unread messages to general, staggered one second apart so they
// sort deterministically and all land strictly past the read frontier.
async function emitUnreadMessages(
  page: import("@playwright/test").Page,
  count: number,
) {
  const base = unreadTimestamp();
  for (let index = 0; index < count; index += 1) {
    await emitMockMessage(
      page,
      "general",
      `Unread message ${index + 1}`,
      base + index,
    );
  }
}

// Emits `count` unread rows inside one page task, for runs too long to emit
// one round trip at a time.
async function emitUnreadMessagesInOnePass(
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
          content: `Unread message ${index + 1}`,
          pubkey,
          createdAt: base + index,
        });
      }
    },
    {
      total: count,
      base: unreadTimestamp(),
      pubkey: TEST_IDENTITIES.alice.pubkey,
    },
  );
}

type SampledFrame = {
  dividerTop: number | null;
  rowsShownWithoutDivider: boolean;
};

// Samples each animation frame: where the "New" divider is (null while it is
// not visibly painted) and whether general's rows painted without it.
async function startFrameSampler(page: import("@playwright/test").Page) {
  await page.evaluate(() => {
    const frames: SampledFrame[] = [];
    const sampler = window as Window & {
      __openAtFrames?: SampledFrame[];
      __openAtStop?: boolean;
    };
    sampler.__openAtFrames = frames;
    sampler.__openAtStop = false;
    const step = () => {
      if (sampler.__openAtStop) return;
      const timeline = document.querySelector<HTMLElement>(
        '[data-testid="message-timeline"]',
      );
      const title = document.querySelector('[data-testid="chat-title"]');
      const visible =
        timeline !== null && getComputedStyle(timeline).visibility !== "hidden";
      const line = visible
        ? document.querySelector<HTMLElement>(
            '[data-testid="message-unread-divider"]',
          )
        : null;
      const rows =
        visible &&
        title?.textContent === "general" &&
        timeline?.querySelector("[data-message-id]") !== null;
      frames.push({
        dividerTop:
          line && timeline
            ? line.getBoundingClientRect().top -
              timeline.getBoundingClientRect().top
            : null,
        rowsShownWithoutDivider: rows && line === null,
      });
      requestAnimationFrame(step);
    };
    requestAnimationFrame(step);
  });
}

async function stopFrameSampler(page: import("@playwright/test").Page) {
  return page.evaluate(() => {
    const sampler = window as Window & {
      __openAtFrames?: SampledFrame[];
      __openAtStop?: boolean;
    };
    sampler.__openAtStop = true;
    return sampler.__openAtFrames ?? [];
  });
}

// Scroll the timeline up so the viewport is no longer pinned to the bottom.
// The pill auto-dismisses once the user reaches the bottom of the timeline, so
// it only stays rendered while scrolled up. Scrolling part-way (rather than to
// the very top) keeps real message context on screen instead of the channel's
// empty-state intro.
async function scrollTimelineUp(page: import("@playwright/test").Page) {
  await page.getByTestId("message-timeline").evaluate((el) => {
    el.scrollTop = Math.floor(el.scrollHeight * 0.35);
  });
  await page.waitForTimeout(300);
}

test.describe("unread pill & divider", () => {
  test("01-opens-at-first-unread", async ({ page }) => {
    await installMockBridge(page);
    await page.goto("/");

    // Open general, then switch to random so general becomes inactive
    await page.getByTestId("channel-general").click();
    await expect(page.getByTestId("chat-title")).toHaveText("general");
    await waitForMockLiveSubscription(page, "general");

    await page.getByTestId("channel-random").click();
    await expect(page.getByTestId("chat-title")).toHaveText("random");

    await emitUnreadMessages(page, 20);

    // Record every frame from the click on: once rows show, the "New" divider
    // must already sit at its final place under the top chrome.
    await startFrameSampler(page);
    await page.getByTestId("channel-general").click();
    await expect(page.getByTestId("chat-title")).toHaveText("general");

    const divider = page.getByTestId("message-unread-divider");
    await expect(divider).toBeVisible();
    await page.waitForTimeout(500);
    const frames = await stopFrameSampler(page);
    const shown = frames.filter((frame) => frame.dividerTop !== null);
    expect(shown.length).toBeGreaterThan(5);
    const first = shown[0].dividerTop ?? 0;
    for (const frame of shown) {
      expect(Math.abs((frame.dividerTop ?? 0) - first)).toBeLessThanOrEqual(1);
    }
    // No frame showed the general timeline before the divider was in place.
    expect(frames.filter((frame) => frame.rowsShownWithoutDivider)).toEqual([]);
    // The divider sits right under the top chrome, not at the bottom.
    const { dividerTop, timelineTop, chrome } = await page.evaluate(() => {
      const timeline = document.querySelector<HTMLElement>(
        '[data-testid="message-timeline"]',
      );
      const line = document.querySelector<HTMLElement>(
        '[data-testid="message-unread-divider"]',
      );
      if (!timeline || !line) throw new Error("missing timeline or divider");
      return {
        dividerTop: line.getBoundingClientRect().top,
        timelineTop: timeline.getBoundingClientRect().top,
        chrome: Number.parseFloat(getComputedStyle(timeline).paddingTop),
      };
    });
    expect(dividerTop - timelineTop).toBeGreaterThanOrEqual(chrome - 1);
    expect(dividerTop - timelineTop).toBeLessThanOrEqual(chrome + 40);
    // The first unread is already on screen, so the pill pointing up to it
    // stays hidden.
    await expect(page.getByTestId("message-unread-pill")).toHaveCount(0);
  });

  test("02-unread-divider-visible", async ({ page }) => {
    await installMockBridge(page);
    await page.goto("/");

    await page.getByTestId("channel-general").click();
    await expect(page.getByTestId("chat-title")).toHaveText("general");
    await waitForMockLiveSubscription(page, "general");

    await page.getByTestId("channel-random").click();
    await expect(page.getByTestId("chat-title")).toHaveText("random");

    await emitUnreadMessages(page, 3);

    await page.getByTestId("channel-general").click();
    await expect(page.getByTestId("chat-title")).toHaveText("general");

    const divider = page.getByTestId("message-unread-divider");
    await expect(divider).toBeVisible();

    // Scroll the divider into view before asserting it.
    await divider.scrollIntoViewIfNeeded();
    await page.waitForTimeout(300);
  });

  test("03-long-unread-run-opens-at-first-unread", async ({ page }) => {
    await installMockBridge(page);
    await page.goto("/");

    await page.getByTestId("channel-general").click();
    await expect(page.getByTestId("chat-title")).toHaveText("general");
    await waitForMockLiveSubscription(page, "general");

    await page.getByTestId("channel-random").click();
    await expect(page.getByTestId("chat-title")).toHaveText("random");

    // A run far longer than one screen still opens at its first row, and
    // "Jump to latest" then takes the reader to the newest one.
    await emitUnreadMessagesInOnePass(page, 260);

    await page.getByTestId("channel-general").click();
    await expect(page.getByTestId("chat-title")).toHaveText("general");
    await expect(page.getByTestId("message-unread-divider")).toBeVisible();
    await expect(
      page.getByText("Unread message 1", { exact: true }),
    ).toBeVisible();
    await expect(page.getByTestId("message-unread-pill")).toHaveCount(0);

    await page.getByRole("button", { name: /jump to latest/i }).click();
    await expect(
      page.getByText("Unread message 260", { exact: true }),
    ).toBeVisible();
  });

  test("04-mark-unread-suppresses-pill", async ({ page }) => {
    await installMockBridge(page);
    await page.goto("/");

    await page.getByTestId("channel-general").click();
    await expect(page.getByTestId("chat-title")).toHaveText("general");

    // Mark channel unread via context menu on the sidebar item
    await page.getByTestId("channel-general").click({ button: "right" });
    await page.getByText("Mark unread").click();

    // Switch away and back to re-open the channel
    await page.getByTestId("channel-random").click();
    await expect(page.getByTestId("chat-title")).toHaveText("random");

    // Forced channel unread uses channel-name emphasis, not the thread dot.
    await expect(page.getByTestId("channel-general")).toHaveCSS(
      "font-weight",
      "700",
    );
    await expect(page.getByTestId("channel-unread-dot-general")).toHaveCount(0);

    await page.getByTestId("channel-general").click();
    await expect(page.getByTestId("chat-title")).toHaveText("general");

    // Pill and divider should NOT appear (suppressed for forced-unread)
    await expect(page.getByTestId("message-unread-pill")).toHaveCount(0);
    await expect(page.getByTestId("message-unread-divider")).toHaveCount(0);
  });
});
