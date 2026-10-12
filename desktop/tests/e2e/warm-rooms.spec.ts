import { expect, test, type Page } from "@playwright/test";

import { TEST_IDENTITIES, installMockBridge } from "../helpers/bridge";

/**
 * Warm rooms: with `warmRooms` > 1 in dev-tuning.json, recently opened rooms
 * stay rendered while hidden, so switching back shows the room at once, at the
 * scroll position it was left at, without reloading it from the relay.
 */

type MockWindow = Window & {
  __BUZZ_E2E_EMIT_MOCK_MESSAGE__?: (input: {
    channelName: string;
    content: string;
    createdAt?: number;
    pubkey?: string;
  }) => unknown;
  __BUZZ_E2E_COMMAND_PAYLOADS__?: Array<{ command: string; payload: unknown }>;
};

const FILLER_COUNT = 80;

async function seedGeneral(page: Page) {
  await page.waitForFunction(
    () =>
      typeof (window as MockWindow).__BUZZ_E2E_EMIT_MOCK_MESSAGE__ ===
      "function",
  );
  await page.evaluate(
    ({ count, alice }) => {
      const emit = (window as MockWindow).__BUZZ_E2E_EMIT_MOCK_MESSAGE__;
      if (!emit) throw new Error("emitter missing");
      const base = Math.floor(Date.now() / 1000) - 10_000;
      for (let i = 0; i < count; i += 1) {
        emit({
          channelName: "general",
          content: `WR filler ${i}\nsecond line\nthird line`,
          createdAt: base + i,
          pubkey: alice,
        });
      }
    },
    { count: FILLER_COUNT, alice: TEST_IDENTITIES.alice.pubkey },
  );
}

function visibleTimeline(page: Page) {
  return page.getByTestId("message-timeline").filter({ visible: true });
}

async function openChannel(page: Page, name: string) {
  await page.getByTestId(`channel-${name}`).click();
  await expect(
    page.getByTestId("chat-title").filter({ visible: true }),
  ).toHaveText(name);
}

async function channelWindowCalls(page: Page, channelId: string) {
  return page.evaluate(
    (id) =>
      ((window as MockWindow).__BUZZ_E2E_COMMAND_PAYLOADS__ ?? []).filter(
        (entry) =>
          entry.command === "get_channel_window" &&
          (entry.payload as { channelId?: string }).channelId === id,
      ).length,
    channelId,
  );
}

test("switching back to a warm room keeps it rendered where it was left", async ({
  page,
}) => {
  await page.addInitScript(() => {
    (
      window as Window & { __BUZZ_E2E_DEV_TUNING__?: string }
    ).__BUZZ_E2E_DEV_TUNING__ = '{"warmRooms":3}';
  });
  await installMockBridge(page);
  await page.goto("/");
  await seedGeneral(page);

  await openChannel(page, "general");
  await expect(
    visibleTimeline(page).getByText(`WR filler ${FILLER_COUNT - 1}`),
  ).toBeVisible();

  // Read back up the room, then leave it.
  const timeline = visibleTimeline(page);
  await timeline.hover();
  for (let i = 0; i < 6; i += 1) await page.mouse.wheel(0, -500);
  await page.waitForTimeout(300);
  const leftAt = await timeline.evaluate((el) => (el as HTMLElement).scrollTop);
  const generalId = await page
    .getByTestId("channel-general")
    .evaluate(() => window.location.hash.match(/channels\/([^/?]+)/)?.[1]);
  expect(generalId).toBeTruthy();

  await openChannel(page, "random");
  // The left room is still in the DOM, hidden.
  await expect(page.getByTestId("message-timeline")).toHaveCount(2);

  const callsBefore = await channelWindowCalls(page, generalId as string);
  await openChannel(page, "general");
  const back = visibleTimeline(page);
  await expect(
    back.getByText("WR filler", { exact: false }).first(),
  ).toBeVisible();
  const returnedAt = await back.evaluate((el) => (el as HTMLElement).scrollTop);
  console.log(`[warm rooms] scrollTop left=${leftAt} returned=${returnedAt}`);
  expect(Math.abs(returnedAt - leftAt)).toBeLessThan(40);
  // Coming back revalidates at most once; it does not rebuild from scratch.
  const callsAfter = await channelWindowCalls(page, generalId as string);
  expect(callsAfter - callsBefore).toBeLessThanOrEqual(1);
});

test("one warm room keeps only the open room rendered", async ({ page }) => {
  await installMockBridge(page);
  await page.goto("/");
  await openChannel(page, "general");
  await openChannel(page, "random");
  await expect(page.getByTestId("message-timeline")).toHaveCount(1);
});

test("a warm room survives round trips and still follows new messages", async ({
  page,
}) => {
  await page.addInitScript(() => {
    (
      window as Window & { __BUZZ_E2E_DEV_TUNING__?: string }
    ).__BUZZ_E2E_DEV_TUNING__ = '{"warmRooms":3}';
  });
  await installMockBridge(page);
  await page.goto("/");
  await seedGeneral(page);

  await openChannel(page, "general");
  await expect(
    visibleTimeline(page).getByText(`WR filler ${FILLER_COUNT - 1}`),
  ).toBeVisible();
  await visibleTimeline(page).hover();
  for (let i = 0; i < 6; i += 1) await page.mouse.wheel(0, -500);
  await page.waitForTimeout(300);
  const leftAt = await visibleTimeline(page).evaluate(
    (el) => (el as HTMLElement).scrollTop,
  );

  // A → B → A → B → A: the hidden room's DOM must never move.
  for (let round = 0; round < 2; round += 1) {
    await openChannel(page, "random");
    await openChannel(page, "general");
  }
  const returnedAt = await visibleTimeline(page).evaluate(
    (el) => (el as HTMLElement).scrollTop,
  );
  console.log(`[warm rooms] round trips left=${leftAt} returned=${returnedAt}`);
  expect(Math.abs(returnedAt - leftAt)).toBeLessThan(40);

  // Back at the bottom, a new message in the re-shown room sticks to it.
  await visibleTimeline(page).evaluate((el) => {
    el.scrollTop = el.scrollHeight;
  });
  await visibleTimeline(page).hover();
  await page.mouse.wheel(0, 4000);
  await page.waitForTimeout(300);
  await page.evaluate((alice) => {
    (window as MockWindow).__BUZZ_E2E_EMIT_MOCK_MESSAGE__?.({
      channelName: "general",
      content: "WR live after return",
      pubkey: alice,
    });
  }, TEST_IDENTITIES.alice.pubkey);
  await expect(
    visibleTimeline(page).getByText("WR live after return"),
  ).toBeInViewport();
});
