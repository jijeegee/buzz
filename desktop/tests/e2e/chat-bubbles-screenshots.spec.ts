/**
 * Chat bubbles: the signed-in user's own messages sit on the right with no
 * avatar or name; everyone else — including agents the user owns — stays on
 * the left.
 *
 * Run: pnpm build:e2e && pnpm exec playwright test --project=smoke \
 *        tests/e2e/chat-bubbles-screenshots.spec.ts
 * Output: test-results/chat-bubbles/
 */
import { expect, type Page, test } from "@playwright/test";

import { waitForAnimations } from "../helpers/animations";
import { installMockBridge, TEST_IDENTITIES } from "../helpers/bridge";

const SHOTS = "test-results/chat-bubbles";

// The mock bridge signs in as this identity.
const ME = "deadbeef".repeat(8);
const OTHER = TEST_IDENTITIES.alice.pubkey;
const MY_AGENT = TEST_IDENTITIES.charlie.pubkey;

const MY_TEXT = "Can you check the deploy before lunch?";
const MY_FOLLOW_UP = "Also send me the table when it's ready.";
const OTHER_TEXT = "Sure, looking now.";
const AGENT_TEXT = "Deploy finished. Summary below.";

const CODE_REPORT = [
  "Build log excerpt:",
  "",
  "```ts",
  "export function computeBubbleWidth(body: string, isOwnMessage: boolean): string { return isOwnMessage ? 'max-w-[75%]' : 'max-w-[92%]'; }",
  "```",
].join("\n");

const TABLE_REPORT = [
  "| Check | Status | Duration | Notes |",
  "| --- | --- | --- | --- |",
  "| typecheck | pass | 41s | no new errors |",
  "| unit tests | pass | 2m 10s | 3 known failures, same as main |",
  "| e2e smoke | pass | 6m 02s | screenshots refreshed |",
].join("\n");

const LONG_REPORT = [
  "## Release report",
  "",
  "The desktop build is ready for review. I verified the installer identity, the relay settings, and the bundle contents. " +
    "Everything matched the previous release except the chat timeline, which now renders each message as a bubble. " +
    "Your own messages are aligned to the right without an avatar, while messages from other people and agents stay on the left. " +
    "Consecutive messages from the same sender share a single avatar and name. Timestamps moved inside the bubble.",
  "",
  "- Installer: verified",
  "- Relay: verified",
  "- Bundle: verified",
].join("\n");

const THREAD_REPLY_MINE = "Thanks — ship it after lunch.";
const THREAD_REPLY_AGENT = "Scheduled for 1:30 PM.";

async function emit(
  page: Page,
  pubkey: string,
  content: string,
  createdAt: number,
  options: { channelName?: string; parentEventId?: string } = {},
): Promise<string | undefined> {
  return page.evaluate(
    ({ pubkey, content, createdAt, channelName, parentEventId }) =>
      window.__BUZZ_E2E_EMIT_MOCK_MESSAGE__?.({
        channelName,
        content,
        createdAt,
        parentEventId,
        pubkey,
      })?.id,
    {
      pubkey,
      content,
      createdAt,
      channelName: options.channelName ?? "general",
      parentEventId: options.parentEventId,
    },
  );
}

async function seedTheme(page: Page, theme: "buzz" | "buzz-dark") {
  await page.addInitScript((value) => {
    window.localStorage.setItem("buzz-theme", value);
  }, theme);
}

async function openSeededChannel(page: Page) {
  await installMockBridge(page, {
    mode: "mock",
    searchProfiles: [
      { pubkey: OTHER, displayName: "Alice" },
      {
        pubkey: MY_AGENT,
        displayName: "My Agent",
        isAgent: true,
        ownerPubkey: ME,
      },
    ],
  });
  await page.goto("/", { waitUntil: "domcontentloaded" });
  await page.getByTestId("channel-general").click();
  await expect(page.getByTestId("chat-title")).toHaveText("general");
  await page.waitForFunction(
    () => typeof window.__BUZZ_E2E_EMIT_MOCK_MESSAGE__ === "function",
  );

  const base = Math.floor(Date.now() / 1000) + 5;
  const rootId = await emit(page, ME, MY_TEXT, base);
  await emit(page, ME, MY_FOLLOW_UP, base + 5);
  await emit(page, OTHER, OTHER_TEXT, base + 30);
  await emit(page, MY_AGENT, AGENT_TEXT, base + 60);
  await emit(page, MY_AGENT, CODE_REPORT, base + 62);
  await emit(page, MY_AGENT, TABLE_REPORT, base + 64);
  await emit(page, MY_AGENT, LONG_REPORT, base + 66);
  await emit(page, ME, "👍", base + 90);
  await emit(page, ME, CODE_REPORT, base + 95);

  await emit(page, MY_AGENT, THREAD_REPLY_AGENT, base + 100, {
    parentEventId: rootId,
  });
  await emit(page, ME, THREAD_REPLY_MINE, base + 110, {
    parentEventId: rootId,
  });

  await expect(
    page.getByTestId("message-row").filter({ hasText: "Release report" }),
  ).toBeVisible();
}

// Every left-hand group opener must show who spoke. Avatar fallbacks paint
// after a 200ms delay, so wait for each one rather than racing the screenshot.
async function expectGroupAvatarsPainted(page: Page) {
  const openers = page
    .locator('[data-testid="message-row"]:not([data-own-message])')
    .filter({ has: page.getByTestId("message-author") });
  const count = await openers.count();
  expect(count).toBeGreaterThan(0);
  for (let index = 0; index < count; index += 1) {
    const avatar = openers.nth(index).getByTestId("message-avatar");
    await expect(
      avatar
        .getByTestId("message-avatar-fallback")
        .or(avatar.getByTestId("message-avatar-image")),
    ).toBeVisible();
  }
}

function rowWith(page: Page, text: string) {
  return page.getByTestId("message-row").filter({ hasText: text }).first();
}

test.describe("chat bubbles", () => {
  test.use({ viewport: { width: 1280, height: 2000 } });

  test("own messages sit right without a profile; agents and others sit left", async ({
    page,
  }) => {
    await openSeededChannel(page);

    const mine = rowWith(page, MY_TEXT);
    await expect(mine).toHaveAttribute("data-own-message", "");
    await expect(mine.getByTestId("message-avatar")).toHaveCount(0);
    await expect(mine.getByTestId("message-author")).toHaveCount(0);

    for (const text of [OTHER_TEXT, AGENT_TEXT]) {
      const row = rowWith(page, text);
      await expect(row).not.toHaveAttribute("data-own-message", /.*/);
      await expect(row.getByTestId("message-avatar")).toHaveCount(1);
    }

    const timeline = await page.getByTestId("message-timeline").boundingBox();
    const myBubble = await mine.getByTestId("message-bubble").boundingBox();
    const agentBubble = await rowWith(page, AGENT_TEXT)
      .getByTestId("message-bubble")
      .boundingBox();
    if (!timeline || !myBubble || !agentBubble) {
      throw new Error("Timeline or bubbles not rendered.");
    }
    const timelineCenter = timeline.x + timeline.width / 2;
    expect(myBubble.x).toBeGreaterThan(timelineCenter);
    expect(agentBubble.x + agentBubble.width).toBeLessThan(timelineCenter);
  });

  test("own code blocks stay inside the bubble", async ({ page }) => {
    await openSeededChannel(page);
    const ownCode = page
      .locator("[data-own-message]")
      .filter({ hasText: "Build log excerpt" })
      .first();
    const bubble = await ownCode.getByTestId("message-bubble").boundingBox();
    const code = await ownCode.locator("pre").first().boundingBox();
    if (!bubble || !code) {
      throw new Error("Own code bubble not rendered.");
    }
    expect(code.x).toBeGreaterThanOrEqual(bubble.x);
    expect(code.x + code.width).toBeLessThanOrEqual(bubble.x + bubble.width);
  });

  test("own thread summary sits under the right-hand bubble", async ({
    page,
  }) => {
    await openSeededChannel(page);
    const summary = page.getByTestId("message-thread-summary").first();
    await expect(summary).toBeVisible();
    const summaryBox = await summary.boundingBox();
    const timeline = await page.getByTestId("message-timeline").boundingBox();
    if (!summaryBox || !timeline) {
      throw new Error("Thread summary not rendered.");
    }
    expect(summaryBox.x).toBeGreaterThan(timeline.x + timeline.width / 2);
  });

  for (const theme of ["buzz", "buzz-dark"] as const) {
    test(`channel and thread screenshots (${theme})`, async ({ page }) => {
      await seedTheme(page, theme);
      await openSeededChannel(page);
      await expectGroupAvatarsPainted(page);
      await waitForAnimations(page);
      await page.screenshot({
        path: `${SHOTS}/channel-${theme}.png`,
        fullPage: true,
      });

      await page.getByTestId("message-thread-summary").first().click();
      await expect(
        page.getByTestId("message-row").filter({ hasText: THREAD_REPLY_MINE }),
      ).toBeVisible();
      await expectGroupAvatarsPainted(page);
      await waitForAnimations(page);
      await page.screenshot({
        path: `${SHOTS}/thread-${theme}.png`,
        fullPage: true,
      });
    });
  }

  test("direct message screenshot", async ({ page }) => {
    await openSeededChannel(page);
    await page.getByTestId("channel-alice-tyler").click();
    const base = Math.floor(Date.now() / 1000) + 5;
    await emit(page, OTHER, "Are you free for a quick call?", base, {
      channelName: "alice-tyler",
    });
    await emit(page, ME, "Give me ten minutes.", base + 20, {
      channelName: "alice-tyler",
    });
    await emit(page, OTHER, "Works for me.", base + 40, {
      channelName: "alice-tyler",
    });
    await expect(rowWith(page, "Works for me.")).toBeVisible();
    await expectGroupAvatarsPainted(page);
    await waitForAnimations(page);
    await page.screenshot({ path: `${SHOTS}/dm.png`, fullPage: true });
  });
});
