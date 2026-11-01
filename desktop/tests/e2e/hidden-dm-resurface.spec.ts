import { expect, type Page, test } from "@playwright/test";

import { installMockBridge, TEST_IDENTITIES } from "../helpers/bridge";

// A hidden DM (kind:30622) is not in the sidebar, but a new message in it
// brings it back. Nothing counts that message: no unread badge and no
// desktop notification, before or after the DM is back.

const HIDDEN_DM = "bob-tyler";
// e2eBridge mock channel id of the bob-tyler DM.
const HIDDEN_DM_ID = "7eb9f239-9393-50b0-bd76-d85eef0511c7";
// How long the mock holds the reopen, so "before it is back" is observable.
const REOPEN_DELAY_MS = 2_000;

async function notificationCount(page: Page) {
  return page.evaluate(
    () =>
      (
        window as Window & {
          __KUZZ_E2E_NOTIFICATIONS__?: unknown[];
        }
      ).__KUZZ_E2E_NOTIFICATIONS__?.length ?? 0,
  );
}

/** Whether an open live REQ asks for the hidden DM's room. */
async function hiddenDmIsLive(page: Page) {
  return page.evaluate((roomId) => {
    const open = window.__KUZZ_E2E_LIVE_REQS__?.() ?? [];
    return open.some(
      (sub) =>
        sub.subIdClass === "live" &&
        sub.filters.some((filter) =>
          ((filter as { "#h"?: string[] })["#h"] ?? []).includes(roomId),
        ),
    );
  }, HIDDEN_DM_ID);
}

test("a message in a hidden DM brings it back, uncounted until then", async ({
  page,
}) => {
  await installMockBridge(page, {
    hiddenDms: [HIDDEN_DM],
    openDmDelayMs: REOPEN_DELAY_MS,
  });
  await page.goto("/");
  await expect(page.getByTestId("channel-general")).toBeVisible();
  await expect(page.getByTestId(`channel-${HIDDEN_DM}`)).toHaveCount(0);
  await expect.poll(() => hiddenDmIsLive(page)).toBe(true);
  const notificationsBefore = await notificationCount(page);

  const sentAt = Date.now();
  await page.evaluate(
    ({ channelName, pubkey }) =>
      window.__KUZZ_E2E_EMIT_MOCK_MESSAGE__?.({
        channelName,
        content: "Are you still there?",
        pubkey,
      }),
    { channelName: HIDDEN_DM, pubkey: TEST_IDENTITIES.bob.pubkey },
  );

  // While the reopen is held: no row, no badge, no notification.
  await page.waitForTimeout(REOPEN_DELAY_MS / 2);
  expect(Date.now() - sentAt).toBeLessThan(REOPEN_DELAY_MS);
  await expect(page.getByTestId(`channel-${HIDDEN_DM}`)).toHaveCount(0);
  await expect(page.getByTestId(`channel-unread-${HIDDEN_DM}`)).toHaveCount(0);
  expect(await notificationCount(page)).toBe(notificationsBefore);

  // Back in the sidebar. The message that brought it back arrived while the
  // DM was not listed, so it is not counted afterwards either (pinned as of
  // A4, so moving resurface onto the room bundle keeps it).
  await expect(page.getByTestId(`channel-${HIDDEN_DM}`)).toBeVisible({
    timeout: 10_000,
  });
  await page.waitForTimeout(1_000);
  await expect(page.getByTestId(`channel-unread-${HIDDEN_DM}`)).toHaveCount(0);
  await expect(page.getByTestId(`channel-unread-dot-${HIDDEN_DM}`)).toHaveCount(
    0,
  );
  expect(await notificationCount(page)).toBe(notificationsBefore);
});
