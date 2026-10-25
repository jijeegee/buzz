import { expect, type Page, test } from "@playwright/test";

import { TEST_IDENTITIES, installMockBridge } from "../helpers/bridge";

type CommitCountWindow = Window & {
  __KUZZ_E2E_COMMIT_COUNTS__?: Record<string, number>;
  __KUZZ_E2E_HAS_MOCK_LIVE_SUBSCRIPTION__?: (input: {
    channelName: string;
    kind?: number;
  }) => boolean;
};

const OTHER_ROOMS = ["random", "engineering", "alice-tyler"] as const;

function channelScreenCommits(page: Page) {
  return page.evaluate(
    () =>
      (window as CommitCountWindow).__KUZZ_E2E_COMMIT_COUNTS__?.ChannelScreen ??
      0,
  );
}

async function waitForLiveSubscription(page: Page, channelName: string) {
  await expect
    .poll(
      () =>
        page.evaluate(
          (name) =>
            (
              window as CommitCountWindow
            ).__KUZZ_E2E_HAS_MOCK_LIVE_SUBSCRIPTION__?.({
              channelName: name,
              kind: 40002,
            }) ?? false,
          channelName,
        ),
      // Background admission is paced across the fixture's channels.
      { timeout: 15_000 },
    )
    .toBe(true);
}

/** Waits until the open room stops committing (boot and open work done). */
async function settledChannelScreenCommits(page: Page) {
  let previous = -1;
  let current = await channelScreenCommits(page);
  while (current !== previous) {
    await page.waitForTimeout(1_000);
    previous = current;
    current = await channelScreenCommits(page);
  }
  return current;
}

test.beforeEach(async ({ page }) => {
  await installMockBridge(page);
});

test("other rooms' live events do not re-render the open room", async ({
  page,
}) => {
  await page.goto("/");
  await page.getByTestId("channel-general").click();
  await expect(page.getByTestId("chat-title")).toHaveText("general");
  await expect(page.getByTestId("message-timeline")).toBeVisible();
  for (const room of OTHER_ROOMS) {
    await waitForLiveSubscription(page, room);
  }
  for (const room of OTHER_ROOMS) {
    await expect(page.getByTestId(`channel-${room}`)).not.toHaveCSS(
      "font-weight",
      "700",
    );
  }

  const baseline = await settledChannelScreenCommits(page);
  expect(baseline).toBeGreaterThan(0);

  // One event at a time, as live traffic arrives, so React cannot batch them.
  for (let index = 0; index < 20; index += 1) {
    await page.evaluate(
      ({ content, pubkey, room }) => {
        window.__KUZZ_E2E_EMIT_MOCK_MESSAGE__?.({
          channelName: room,
          content,
          kind: 40002,
          pubkey,
        });
      },
      {
        content: `Other room event ${index + 1}`,
        pubkey: TEST_IDENTITIES.alice.pubkey,
        room: OTHER_ROOMS[index % OTHER_ROOMS.length],
      },
    );
    await page.waitForTimeout(100);
  }

  // The sidebar still reflects every other room's new activity.
  for (const room of OTHER_ROOMS) {
    await expect(page.getByTestId(`channel-${room}`)).toHaveCSS(
      "font-weight",
      "700",
    );
  }
  expect(await settledChannelScreenCommits(page)).toBe(baseline);
});
