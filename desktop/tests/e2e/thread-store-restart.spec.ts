import { expect, type Page, test } from "@playwright/test";

import { installMockBridge } from "../helpers/bridge";

const GENERAL_CHANNEL_ID = "9a1657ac-f7aa-5db0-b632-d8bbeb6dfb50";
const ROOT_ID = "5a".repeat(32);
const ROOT_TEXT = "stored thread root";
const REPLY_TEXT = "stored thread reply";
const STORE_PREFIX = "kuzz-e2e-message-store:";

type MockRange = { loIsStart: boolean; hiAt: number };
type MockState = {
  threadCoverage: Record<string, MockRange[]>;
  summaries: Record<string, { createdAt: number; payload: unknown }>;
};
type EmitWindow = Window & {
  __KUZZ_E2E_EMIT_MOCK_MESSAGE__?: (input: {
    channelName: string;
    content: string;
    parentEventId?: string;
    createdAt?: number;
    id?: string;
  }) => unknown;
  __KUZZ_E2E_COMMAND_PAYLOADS__?: Array<{ command: string; payload: unknown }>;
};

function threadPanel(page: Page) {
  return page.getByTestId("message-thread-panel");
}

async function openThread(page: Page) {
  await page
    .locator(
      `[data-testid="message-thread-summary"][data-thread-head-id="${ROOT_ID}"]`,
    )
    .first()
    .click();
  await expect(threadPanel(page)).toBeVisible();
}

/** One session: open a thread only this session's relay has. */
async function storeThread(page: Page) {
  await installMockBridge(page);
  await page.goto("/");
  await page.waitForFunction(
    () =>
      typeof (window as EmitWindow).__KUZZ_E2E_EMIT_MOCK_MESSAGE__ ===
      "function",
  );
  await page.evaluate(
    ({ rootId, rootText, replyText }) => {
      const emit = (window as EmitWindow).__KUZZ_E2E_EMIT_MOCK_MESSAGE__;
      if (!emit) throw new Error("emitter missing");
      // Older than the restart catch-up's lookback, so only the thread's own
      // claims can prove its replies.
      const base = Math.floor(Date.now() / 1000) - 4_000;
      emit({
        channelName: "general",
        content: rootText,
        createdAt: base,
        id: rootId,
      });
      for (let index = 0; index < 3; index += 1) {
        emit({
          channelName: "general",
          content: `${replyText} ${index}`,
          createdAt: base + 10 + index,
          parentEventId: rootId,
        });
      }
    },
    { rootId: ROOT_ID, rootText: ROOT_TEXT, replyText: REPLY_TEXT },
  );
  await page.getByTestId("channel-general").click();
  await expect(page.getByText(ROOT_TEXT)).toBeVisible();
  await openThread(page);
  await expect(threadPanel(page).getByText(`${REPLY_TEXT} 2`)).toBeVisible();
  // Wait until the store holds the thread page's claim.
  await expect
    .poll(() =>
      page.evaluate(
        ({ prefix, rootId }) =>
          Object.entries(window.localStorage).some(
            ([key, value]) =>
              key.startsWith(prefix) &&
              ((JSON.parse(value) as MockState).threadCoverage[rootId]
                ?.length ?? 0) > 0,
          ),
        { prefix: STORE_PREFIX, rootId: ROOT_ID },
      ),
    )
    .toBe(true);
  const saved = await page.evaluate(
    (prefix) =>
      Object.fromEntries(
        Object.entries(window.localStorage).filter(([key]) =>
          key.startsWith(prefix),
        ),
      ),
    STORE_PREFIX,
  );
  // The relay's windows carry a 39005 summary per thread root, which the
  // store keeps; the mock relay sends none, so store the one it would have.
  return Object.fromEntries(
    Object.entries(saved).map(([key, value]) => {
      const state = JSON.parse(value) as MockState;
      const now = Math.floor(Date.now() / 1000);
      state.summaries[ROOT_ID] = {
        createdAt: now,
        payload: {
          id: "5b".repeat(32),
          pubkey: "5c".repeat(32),
          created_at: now,
          kind: 39005,
          tags: [
            ["h", GENERAL_CHANNEL_ID],
            ["e", ROOT_ID],
          ],
          content: JSON.stringify({
            reply_count: 3,
            descendant_count: 3,
            last_reply_at: now - 3_988,
            participants: [],
          }),
          sig: "",
        },
      };
      return [key, JSON.stringify(state)];
    }),
  );
}

/** Restart with `store`; the relay's thread windows are held for 5 s. */
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
      mock: { ...testWindow.__KUZZ_E2E__?.mock, threadRepliesDelayMs: 5_000 },
    };
  }, store);
  // A fresh start at the root route: nothing reopens the thread by itself.
  await page.goto("/");
  await page.getByTestId("channel-general").click();
  await expect(page.getByText(ROOT_TEXT)).toBeVisible();
  await expect(threadPanel(page)).toHaveCount(0);
  await page.evaluate(() => {
    const log = (window as EmitWindow).__KUZZ_E2E_COMMAND_PAYLOADS__;
    if (log) log.length = 0;
  });
}

/** Per animation frame: is a thread skeleton showing, and where is the reply. */
async function startFrameSampler(page: Page, text: string) {
  await page.evaluate((needle) => {
    const sampler = window as Window & {
      __threadFrames?: { skeleton: boolean; replyTop: number | null }[];
      __threadStop?: boolean;
    };
    sampler.__threadFrames = [];
    sampler.__threadStop = false;
    const step = () => {
      if (sampler.__threadStop) return;
      const panel = document.querySelector<HTMLElement>(
        '[data-testid="message-thread-panel"]',
      );
      const body = document.querySelector<HTMLElement>(
        '[data-testid="message-thread-body"]',
      );
      const skeleton = panel
        ? [
            ...panel.querySelectorAll<HTMLElement>(
              '.timeline-skeleton-grace, [data-testid="message-thread-loading"]',
            ),
          ].some(
            (element) => getComputedStyle(element).visibility === "visible",
          )
        : false;
      const reply = body
        ? [...body.querySelectorAll<HTMLElement>("*")].find(
            (element) =>
              element.childElementCount === 0 &&
              element.textContent?.trim() === needle,
          )
        : undefined;
      sampler.__threadFrames?.push({
        skeleton,
        replyTop:
          reply && body
            ? Math.round(
                reply.getBoundingClientRect().top -
                  body.getBoundingClientRect().top,
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
      __threadFrames?: { skeleton: boolean; replyTop: number | null }[];
      __threadStop?: boolean;
    };
    sampler.__threadStop = true;
    return sampler.__threadFrames ?? [];
  });
}

async function threadWindowCalls(page: Page) {
  return page.evaluate(
    (rootId) =>
      ((window as EmitWindow).__KUZZ_E2E_COMMAND_PAYLOADS__ ?? []).filter(
        (entry) =>
          entry.command === "get_thread_windows" &&
          JSON.stringify(entry.payload).includes(rootId),
      ).length,
    ROOT_ID,
  );
}

test("restart opens a thread at its final position from proven stored replies", async ({
  page,
}) => {
  const store = await storeThread(page);
  await restartWithStore(page, store);
  const newest = `${REPLY_TEXT} 2`;
  await startFrameSampler(page, newest);
  await openThread(page);
  // The relay's thread window is held for 5 s and lacks these replies: they
  // came from the store.
  await expect(threadPanel(page).getByText(newest)).toBeVisible({
    timeout: 2_000,
  });
  await page.waitForTimeout(1_000);
  const frames = await stopFrameSampler(page);
  expect(await threadWindowCalls(page)).toBe(0);

  const first = frames.findIndex((frame) => frame.replyTop !== null);
  expect(first).toBeGreaterThanOrEqual(0);
  // No skeleton over proven replies, and the newest reply never moves or
  // disappears from its first visible frame on.
  expect(frames.some((frame) => frame.skeleton)).toBe(false);
  const tops = frames.slice(first).map((frame) => frame.replyTop);
  expect(tops.length).toBeGreaterThan(10);
  expect(new Set(tops)).toEqual(new Set([tops[0]]));
});

test("restart never paints stored replies the store cannot prove", async ({
  page,
}) => {
  const store = await storeThread(page);
  // Drop the thread's claims: the stored replies prove nothing now (the
  // restart catch-up's complete run starts after them).
  const cut = Object.fromEntries(
    Object.entries(store).map(([key, value]) => {
      const state = JSON.parse(value) as MockState;
      delete state.threadCoverage[ROOT_ID];
      return [key, JSON.stringify(state)];
    }),
  );
  await restartWithStore(page, cut);
  await startFrameSampler(page, `${REPLY_TEXT} 2`);
  await openThread(page);
  // The relay reset on reload, so it answers without these replies.
  await expect.poll(() => threadWindowCalls(page)).toBeGreaterThan(0);
  await page.waitForTimeout(6_000);
  const frames = await stopFrameSampler(page);
  expect(frames.every((frame) => frame.replyTop === null)).toBe(true);
});
