/**
 * Inbox and Chats are one screen: an inbox row enters the real channel screen
 * and the inbox list stays pulled out beside it; the sidebar's Inbox button
 * hides and shows that list on a channel.
 *
 * Run: pnpm build:e2e && pnpm exec playwright test --project=smoke \
 *        tests/e2e/inbox-panel-screenshots.spec.ts
 * Output: test-results/inbox-panel/
 */
import { expect, type Page, test } from "@playwright/test";

import { waitForAnimations } from "../helpers/animations";
import { installMockBridge, TEST_IDENTITIES } from "../helpers/bridge";

const SHOTS = "test-results/inbox-panel";
const GENERAL_CHANNEL_ID = "9a1657ac-f7aa-5db0-b632-d8bbeb6dfb50";

type MockFeedWindow = Window & {
  __BUZZ_E2E_EMIT_MOCK_MESSAGE__?: (input: {
    channelName: string;
    content: string;
    createdAt?: number;
    id?: string;
    parentEventId?: string;
    pubkey?: string;
  }) => {
    content: string;
    created_at: number;
    id: string;
    kind: number;
    pubkey: string;
    tags: string[][];
  };
  __BUZZ_E2E_PUSH_MOCK_FEED_ITEM__?: (item: {
    category: "mention" | "needs_action" | "activity" | "agent_activity";
    channel_id: string | null;
    channel_name: string;
    channel_type?: string | null;
    content: string;
    created_at: number;
    id: string;
    kind: number;
    pubkey: string;
    tags: string[][];
  }) => void;
};

async function seedGeneralMessages(page: Page) {
  await expect(page.getByTestId("home-inbox-list")).toBeVisible({
    timeout: 10_000,
  });
  await page.waitForFunction(() => {
    const win = window as MockFeedWindow;
    return (
      typeof win.__BUZZ_E2E_EMIT_MOCK_MESSAGE__ === "function" &&
      typeof win.__BUZZ_E2E_PUSH_MOCK_FEED_ITEM__ === "function"
    );
  });
  await page.evaluate(
    ({ channelId, senderPubkey }) => {
      const win = window as MockFeedWindow;
      const emit = win.__BUZZ_E2E_EMIT_MOCK_MESSAGE__;
      const push = win.__BUZZ_E2E_PUSH_MOCK_FEED_ITEM__;
      if (!emit || !push) throw new Error("Mock bridge helpers missing.");
      const now = Math.floor(Date.now() / 1000);
      const root = emit({
        channelName: "general",
        content: "Release checklist is ready for review.",
        createdAt: now - 60,
        id: "panel-root",
        pubkey: senderPubkey,
      });
      const reply = emit({
        channelName: "general",
        content: "Left two comments on the checklist thread.",
        createdAt: now - 30,
        id: "panel-reply",
        parentEventId: root.id,
        pubkey: senderPubkey,
      });
      for (const event of [root, reply]) {
        push({
          category: "mention",
          channel_id: channelId,
          channel_name: "general",
          channel_type: "stream",
          content: event.content,
          created_at: event.created_at,
          id: event.id,
          kind: event.kind,
          pubkey: event.pubkey,
          tags: event.tags,
        });
      }
    },
    {
      channelId: GENERAL_CHANNEL_ID,
      senderPubkey: TEST_IDENTITIES.alice.pubkey,
    },
  );
}

test.describe("inbox panel beside the chat screen", () => {
  test.use({ viewport: { width: 1440, height: 900 } });

  test("rows enter the real chat screen and the sidebar toggles the list", async ({
    page,
  }) => {
    page.on("pageerror", (err) => console.error("PAGE ERROR:", err.message));
    await installMockBridge(page, { mode: "mock" });
    await page.goto("/", { waitUntil: "domcontentloaded" });
    await seedGeneralMessages(page);

    const inboxRow = (text: string) =>
      page
        .locator('[data-testid^="home-inbox-item-"]')
        .filter({ hasText: text })
        .first();
    const replyRow = inboxRow("Left two comments on the checklist thread.");
    await expect(replyRow).toBeVisible({ timeout: 10_000 });
    await waitForAnimations(page);
    await page.screenshot({ path: `${SHOTS}/01-inbox-page.png` });

    // A thread reply enters its channel with the thread maximized.
    await replyRow.click();
    await expect(page).toHaveURL(new RegExp(`/channels/${GENERAL_CHANNEL_ID}`));
    await expect(page.getByTestId("inbox-panel")).toBeVisible();
    await expect(page.getByTestId("focus-thread-drawer-overlay")).toBeVisible({
      timeout: 10_000,
    });
    await waitForAnimations(page);
    await page.screenshot({ path: `${SHOTS}/02-thread-from-inbox.png` });

    // The sidebar Inbox button hides the list; the chat screen stays.
    const inboxButton = page
      .getByTestId("sidebar-primary-menu")
      .getByRole("button", { name: "Inbox", exact: true });
    await inboxButton.click();
    await expect(page.getByTestId("inbox-panel")).toHaveCount(0);
    await expect(page).toHaveURL(new RegExp(`/channels/${GENERAL_CHANNEL_ID}`));
    await waitForAnimations(page);
    await page.screenshot({ path: `${SHOTS}/03-panel-hidden.png` });

    await inboxButton.click();
    await expect(page.getByTestId("inbox-panel")).toBeVisible();
    await waitForAnimations(page);
    await page.screenshot({ path: `${SHOTS}/04-panel-shown.png` });
  });
});

test.describe("inbox rows land exactly where they point", () => {
  test.use({ viewport: { width: 1440, height: 900 } });

  test("re-clicking a thread keeps it open; a channel row lands on the newest message", async ({
    page,
  }) => {
    await page.addInitScript(() => {
      window.localStorage.setItem("buzz.desktop.inbox-filter", "conversations");
    });
    await installMockBridge(page, { mode: "mock" });
    await page.goto("/", { waitUntil: "domcontentloaded" });
    await seedGeneralMessages(page);

    // Make #general long enough to scroll, ending in a message the inbox
    // never lists: "the channel's newest" must mean the room, not the feed.
    await page.evaluate(() => {
      const emit = (window as MockFeedWindow).__BUZZ_E2E_EMIT_MOCK_MESSAGE__;
      if (!emit) throw new Error("Mock bridge helpers missing.");
      const now = Math.floor(Date.now() / 1000);
      for (let index = 0; index < 40; index += 1) {
        emit({
          channelName: "general",
          content: `Filler message ${index}`,
          createdAt: now - 20 + index * 0.1,
        });
      }
      emit({
        channelName: "general",
        content: "Newest message in general",
        createdAt: now + 5,
      });
    });

    const inboxRow = (text: string) =>
      page
        .locator('[data-testid^="home-inbox-item-"]')
        .filter({ hasText: text })
        .first();
    const threadRow = inboxRow("Left two comments on the checklist thread.");
    await threadRow.click();
    const drawer = page.getByTestId("focus-thread-drawer-overlay");
    await expect(drawer).toBeVisible({ timeout: 10_000 });

    // Clicking the same row again lands there again, it never toggles.
    await page
      .getByTestId("inbox-panel")
      .locator('[data-testid^="home-inbox-item-"]')
      .filter({ hasText: "Left two comments on the checklist thread." })
      .first()
      .click();
    await expect(drawer).toBeVisible({ timeout: 10_000 });

    // The channel row lands on the room's newest message with no thread open.
    await page
      .getByTestId("inbox-panel")
      .locator('[data-testid^="home-inbox-item-"]')
      .filter({ hasNotText: "Left two comments" })
      .filter({ hasText: "#general" })
      .first()
      .click();
    await expect(drawer).toHaveCount(0);
    await expect(
      page
        .getByTestId("message-timeline")
        .getByText("Newest message in general"),
    ).toBeInViewport({ timeout: 10_000 });
    await waitForAnimations(page);
    await page.screenshot({ path: `${SHOTS}/05-channel-row-latest.png` });
  });
});

test.describe("Channels + Threads is a chat list", () => {
  test.use({ viewport: { width: 1440, height: 900 } });

  test("my own latest message brings its room to the top", async ({ page }) => {
    await page.addInitScript(() => {
      window.localStorage.setItem("buzz.desktop.inbox-filter", "conversations");
    });
    await installMockBridge(page, { mode: "mock" });
    await page.goto("/", { waitUntil: "domcontentloaded" });
    await expect(page.getByTestId("home-inbox-list")).toBeVisible({
      timeout: 10_000,
    });
    await page.waitForFunction(
      () =>
        typeof (window as MockFeedWindow).__BUZZ_E2E_EMIT_MOCK_MESSAGE__ ===
        "function",
    );

    // A message I send never reaches the mention feed; the chat list must
    // still show its room, newest first.
    await page.evaluate(() => {
      const emit = (window as MockFeedWindow).__BUZZ_E2E_EMIT_MOCK_MESSAGE__;
      if (!emit) throw new Error("Mock bridge helpers missing.");
      emit({
        channelName: "random",
        content: "My own note in random",
        createdAt: Math.floor(Date.now() / 1000) + 30,
        pubkey: "deadbeef".repeat(8),
      });
    });
    // The live update moves the room's last-message time, which refetches
    // just that room.

    const firstRow = page
      .getByTestId("home-inbox-list")
      .locator('[data-testid^="home-inbox-item-"]')
      .first();
    await expect(firstRow).toContainText("My own note in random", {
      timeout: 10_000,
    });
    await expect(firstRow).toContainText("#random");
    await waitForAnimations(page);
    await page.screenshot({ path: `${SHOTS}/06-own-message-room.png` });
  });
});
