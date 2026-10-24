import { expect, test } from "@playwright/test";

import { installMockBridge } from "../helpers/bridge";

const OPENED_ROW = "random room before closing";
const CLOSED_ROW = "arrived while the room was closed";
const REPLY_ROW = "own reply after reopening";

test("a row that arrives while a visited room is closed shows at once on reopen and stays", async ({
  page,
}) => {
  await installMockBridge(page);
  await page.goto("/");
  await page.waitForFunction(
    () => typeof window.__KUZZ_E2E_EMIT_MOCK_MESSAGE__ === "function",
  );
  // Timeline rows only: the chat list previews the latest message too.
  const row = (text: string) =>
    page.getByTestId("message-row").filter({ hasText: text });
  // Signed in the past, so the reopened room's live subscription (which
  // starts at "now") cannot replay them: the only copy is the local store.
  const startedAt = Math.floor(Date.now() / 1_000);
  const emit = (content: string, secondsAgo: number) =>
    page.evaluate(
      ({ text, createdAt }) => {
        window.__KUZZ_E2E_EMIT_MOCK_MESSAGE__?.({
          channelName: "random",
          content: text,
          createdAt,
        });
      },
      { text: content, createdAt: startedAt - secondsAgo },
    );

  await page.getByTestId("channel-random").click();
  await emit(OPENED_ROW, 60);
  await expect(row(OPENED_ROW)).toBeVisible();

  await page.getByTestId("channel-general").click();
  await expect(row(OPENED_ROW)).toHaveCount(0);
  await emit(CLOSED_ROW, 30);

  // Hold every newest-window fetch from here on, so what reopening shows can
  // only come from the room's local store.
  await page.evaluate(() => {
    const config = (
      window as Window & {
        __KUZZ_E2E__?: { mock?: Record<string, unknown> };
      }
    ).__KUZZ_E2E__;
    if (!config) throw new Error("E2E config is missing.");
    config.mock = { ...config.mock, channelHeadDelayMs: 8_000 };
  });
  const headFetchesHeld = () =>
    page.evaluate(
      () =>
        (window as Window & { __CHANNEL_WINDOW_INFLIGHT__?: number })
          .__CHANNEL_WINDOW_INFLIGHT__ ?? 0,
    );
  await page.getByTestId("channel-random").click();
  await expect.poll(headFetchesHeld).toBeGreaterThan(0);
  await expect(row(CLOSED_ROW)).toBeVisible({ timeout: 2_000 });

  // Replying re-projects the timeline from the window store. A row kept only
  // in the rendered cache was dropped here until the refetch landed.
  await page.getByTestId("message-input").fill(REPLY_ROW);
  await page.getByTestId("send-message").click();
  await expect(row(REPLY_ROW)).toBeVisible({ timeout: 2_000 });
  await expect(row(CLOSED_ROW)).toBeVisible({ timeout: 500 });
  await expect(row(OPENED_ROW)).toBeVisible();
  expect(await headFetchesHeld()).toBeGreaterThan(0);
});
