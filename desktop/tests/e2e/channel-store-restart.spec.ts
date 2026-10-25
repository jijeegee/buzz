import { expect, type Page, test } from "@playwright/test";

import { installMockBridge } from "../helpers/bridge";

const STORED_ONLY = "stored restart row";
const STORE_PREFIX = "kuzz-e2e-message-store:";

type MockRange = { loIsStart: boolean; hiAt: number };
type MockState = {
  coverage: Record<string, { top: MockRange[]; all: MockRange[] }>;
};

/** One session: open #general with a row only this session's relay has. */
async function storeGeneralWithStoredOnlyRow(page: Page) {
  await installMockBridge(page);
  await page.goto("/");
  await page.waitForFunction(
    () => typeof window.__KUZZ_E2E_EMIT_MOCK_MESSAGE__ === "function",
  );
  await page.evaluate((content) => {
    window.__KUZZ_E2E_EMIT_MOCK_MESSAGE__?.({
      channelName: "general",
      content,
    });
  }, STORED_ONLY);
  await page.getByTestId("channel-general").click();
  await expect(page.getByText(STORED_ONLY)).toBeVisible();
  // Wait until the store holds the row and the opened window's claim.
  await expect
    .poll(() =>
      page.evaluate(
        ({ prefix, needle }) =>
          Object.entries(window.localStorage).some(([key, value]) => {
            if (!key.startsWith(prefix) || !value.includes(needle)) {
              return false;
            }
            const state = JSON.parse(value) as MockState;
            return Object.values(state.coverage).some((room) =>
              room.top.some((range) => range.loIsStart),
            );
          }),
        { prefix: STORE_PREFIX, needle: STORED_ONLY },
      ),
    )
    .toBe(true);
  return page.evaluate(
    (prefix) =>
      Object.fromEntries(
        Object.entries(window.localStorage).filter(([key]) =>
          key.startsWith(prefix),
        ),
      ),
    STORE_PREFIX,
  );
}

/**
 * Restart with the given store and server windows held for 5 s, so anything
 * painted sooner came from the local store.
 */
async function restartWithStore(page: Page, store: Record<string, string>) {
  await page.addInitScript((saved) => {
    for (const [key, value] of Object.entries(saved)) {
      window.localStorage.setItem(key, value);
    }
    const testWindow = window as Window & {
      __KUZZ_E2E__?: { mock?: Record<string, unknown> };
    };
    testWindow.__KUZZ_E2E__ = {
      ...testWindow.__KUZZ_E2E__,
      mock: { ...testWindow.__KUZZ_E2E__?.mock, channelHeadDelayMs: 5_000 },
    };
  }, store);
  await page.reload();
  await expect(page.getByTestId("channel-general")).toBeVisible();
}

/** Per animation frame: is the skeleton showing, and where is the row. */
async function startFrameSampler(page: Page, text: string) {
  await page.evaluate((needle) => {
    const sampler = window as Window & {
      __storeFrames?: { skeleton: boolean; rowTop: number | null }[];
      __storeStop?: boolean;
    };
    sampler.__storeFrames = [];
    sampler.__storeStop = false;
    const step = () => {
      if (sampler.__storeStop) return;
      const timeline = document.querySelector<HTMLElement>(
        '[data-testid="message-timeline"]',
      );
      const skeleton = [
        ...document.querySelectorAll<HTMLElement>(".timeline-skeleton-grace"),
      ].some((element) => getComputedStyle(element).visibility === "visible");
      const row = timeline
        ? [...timeline.querySelectorAll<HTMLElement>("[data-message-id]")].find(
            (element) => element.textContent?.includes(needle),
          )
        : undefined;
      sampler.__storeFrames?.push({
        skeleton,
        rowTop:
          row && timeline
            ? Math.round(
                row.getBoundingClientRect().top -
                  timeline.getBoundingClientRect().top,
              )
            : null,
      });
      requestAnimationFrame(step);
    };
    requestAnimationFrame(step);
  }, text);
}

async function stopFrameSampler(page: Page) {
  return page.evaluate(() => {
    const sampler = window as Window & {
      __storeFrames?: { skeleton: boolean; rowTop: number | null }[];
      __storeStop?: boolean;
    };
    sampler.__storeStop = true;
    return sampler.__storeFrames ?? [];
  });
}

test("restart opens a room at its final position from proven stored rows", async ({
  page,
}) => {
  const store = await storeGeneralWithStoredOnlyRow(page);
  await restartWithStore(page, store);
  await startFrameSampler(page, STORED_ONLY);
  await page.getByTestId("channel-general").click();
  // The server window is held for 5 s: this row came from the store.
  await expect(page.getByText(STORED_ONLY)).toBeVisible({ timeout: 2_000 });
  await page.waitForTimeout(1_000);
  const frames = await stopFrameSampler(page);

  const first = frames.findIndex((frame) => frame.rowTop !== null);
  expect(first).toBeGreaterThanOrEqual(0);
  // No skeleton over proven rows, and the rows never disappear or move once
  // mounted: no server answer replaces them. The list's own one-frame bottom
  // scroll on mount (main does it on any room switch) is the open-position
  // stage's to remove; this check starts after it.
  expect(frames.some((frame) => frame.skeleton)).toBe(false);
  const tops = frames.slice(first + 1).map((frame) => frame.rowTop);
  expect(tops.length).toBeGreaterThan(10);
  expect(new Set(tops)).toEqual(new Set([tops[0]]));
});

test("restart never paints a stored head that stops at a coverage gap", async ({
  page,
}) => {
  const store = await storeGeneralWithStoredOnlyRow(page);
  // Cut the stored room off from its start: the stored head now ends at a
  // gap, so it cannot prove the room's newest page.
  const cut = Object.fromEntries(
    Object.entries(store).map(([key, value]) => {
      const state = JSON.parse(value) as MockState;
      for (const coverage of Object.values(state.coverage)) {
        for (const range of [...coverage.top, ...coverage.all]) {
          range.loIsStart = false;
        }
      }
      return [key, JSON.stringify(state)];
    }),
  );
  await restartWithStore(page, cut);
  const callsBeforeOpen = await page.evaluate(
    () =>
      (window.__KUZZ_E2E_COMMANDS__ ?? []).filter(
        (command) => command === "get_channel_window",
      ).length,
  );

  await startFrameSampler(page, STORED_ONLY);
  await page.getByTestId("channel-general").click();
  // The relay reset on reload, so its window lacks the stored-only row.
  await expect
    .poll(
      () =>
        page.evaluate(
          (before) =>
            (window.__KUZZ_E2E_COMMANDS__ ?? []).filter(
              (command) => command === "get_channel_window",
            ).length - before,
          callsBeforeOpen,
        ),
      { timeout: 3_000 },
    )
    .toBeGreaterThan(0);
  await expect(
    page.getByTestId("message-timeline").locator("[data-message-id]").first(),
  ).toBeVisible({ timeout: 10_000 });
  await page.waitForTimeout(500);
  const frames = await stopFrameSampler(page);
  expect(frames.every((frame) => frame.rowTop === null)).toBe(true);
});
