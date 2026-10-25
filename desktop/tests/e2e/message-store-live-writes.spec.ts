import { expect, type Page, test } from "@playwright/test";

import { installMockBridge } from "../helpers/bridge";

const LIVE_ONLY = "live row kept by the local store";
const SCOPE = { pubkey: "deadbeef".repeat(8), relayUrl: "ws://localhost:3000" };

type StoreWindow = { rows: Array<{ id: string; content: string }> };
type StoreCoverage = Array<{
  ranges: Array<{ class: string; loAt: number; hiAt: number }>;
}>;

function invoke<T>(page: Page, command: string, args: Record<string, unknown>) {
  return page.evaluate(
    ([name, payload]) =>
      (
        window as unknown as {
          __TAURI_INTERNALS__: {
            invoke: (command: string, args: unknown) => Promise<unknown>;
          };
        }
      ).__TAURI_INTERNALS__.invoke(name, payload),
    [command, args] as const,
  ) as Promise<T>;
}

test("live room events reach the local store and survive a restart", async ({
  page,
}) => {
  await installMockBridge(page);
  await page.goto("/");
  await page.waitForFunction(
    () => typeof window.__KUZZ_E2E_EMIT_MOCK_MESSAGE__ === "function",
  );
  await page.getByTestId("channel-general").click();
  await expect
    .poll(() =>
      page.evaluate(() =>
        Boolean(
          window.__KUZZ_E2E_HAS_MOCK_LIVE_SUBSCRIPTION__?.({
            channelName: "general",
            kind: 9,
            exactChannel: true,
          }),
        ),
      ),
    )
    .toBe(true);

  const event = await page.evaluate(
    (content) =>
      window.__KUZZ_E2E_EMIT_MOCK_MESSAGE__?.({
        channelName: "general",
        content,
      }),
    LIVE_ONLY,
  );
  expect(event?.id).toMatch(/^[0-9a-f]{64}$/);
  await expect(page.getByText(LIVE_ONLY)).toBeVisible();
  const channelId = event?.tags.find((tag) => tag[0] === "h")?.[1];
  expect(channelId).toBeTruthy();

  // Restart: the mock relay forgets the row, the local store keeps it, and
  // the live segment left open is closed at the last frame on boot.
  await page.reload();
  await page.waitForFunction(
    () => typeof window.__KUZZ_E2E_EMIT_MOCK_MESSAGE__ === "function",
  );
  const stored = await invoke<StoreWindow>(page, "message_store_read_window", {
    scope: SCOPE,
    channelId,
    before: null,
    limit: 50,
  });
  expect(stored.rows.map((row) => row.id)).toContain(event?.id);
  const [coverage] = await invoke<StoreCoverage>(
    page,
    "message_store_coverage",
    { scope: SCOPE, channelIds: [channelId] },
  );
  const proven = coverage.ranges.filter((range) => range.class === "all");
  expect(proven.length).toBeGreaterThan(0);
  expect(
    proven.some(
      (range) =>
        range.loAt <= (event?.created_at ?? 0) &&
        range.hiAt >= (event?.created_at ?? 0),
    ),
  ).toBe(true);
});
