import { expect, test, type Locator, type Page } from "@playwright/test";

import { KIND_HUDDLE_STARTED } from "../../src/shared/constants/kinds";
import { installMockBridge } from "../helpers/bridge";

const GENERAL_CHANNEL_ID = "9a1657ac-f7aa-5db0-b632-d8bbeb6dfb50";

async function latestClipboardWrite(page: Page) {
  return page.evaluate(() =>
    (window.__KUZZ_E2E_COMMAND_LOG__ ?? []).findLast(
      ({ command }) => command === "copy_text_to_clipboard",
    ),
  );
}

async function expectCopyLinkUnavailable(row: Locator, messageId: string) {
  await row.hover();
  await expect(row.getByTestId(`copy-link-message-${messageId}`)).toHaveCount(
    0,
  );

  const moreActions = row.getByTestId(`more-actions-${messageId}`);
  if (await moreActions.count()) {
    await moreActions.click({ force: true });
    await expect(
      row.page().getByTestId(`copy-message-link-${messageId}`),
    ).toHaveCount(0);
    await row.page().keyboard.press("Escape");
  }
}

test.beforeEach(async ({ page }) => {
  await page.context().grantPermissions(["clipboard-read", "clipboard-write"], {
    origin: "http://127.0.0.1:4173",
  });
  await installMockBridge(page);
});

test("message action rail copies the same canonical thread link as More", async ({
  page,
}) => {
  await page.setViewportSize({ width: 900, height: 700 });
  await page.goto("/");
  await page.getByTestId("channel-general").click();
  await expect(page.getByTestId("chat-title")).toHaveText("general");
  await expect
    .poll(() =>
      page.evaluate(
        () => typeof window.__KUZZ_E2E_EMIT_MOCK_MESSAGE__ === "function",
      ),
    )
    .toBe(true);

  const { replyId, rootId } = await page.evaluate(() => {
    const emit = window.__KUZZ_E2E_EMIT_MOCK_MESSAGE__;
    if (!emit) throw new Error("Mock message emitter is unavailable.");
    const root = emit({
      channelName: "general",
      content: "Copy-link regression root",
      id: "a".repeat(64),
    });
    const reply = emit({
      channelName: "general",
      content: "Copy-link regression reply",
      id: "b".repeat(64),
      parentEventId: root.id,
    });
    return { replyId: reply.id, rootId: root.id };
  });

  await page
    .locator(
      `[data-testid="message-thread-summary"][data-thread-head-id="${rootId}"]`,
    )
    .click();
  const threadPanel = page.getByTestId("message-thread-panel");
  const replyRow = threadPanel.locator(`[data-message-id="${replyId}"]`);
  await expect(replyRow).toContainText("Copy-link regression reply");
  await replyRow.hover();

  const actionBar = replyRow.getByTestId(`message-action-bar-${replyId}`);
  const orderedActionNames = await actionBar
    .getByRole("button")
    .evaluateAll((buttons) =>
      buttons.map((button) => button.getAttribute("aria-label")),
    );
  expect(orderedActionNames).toEqual([
    "React with :+1:",
    "React with :heart:",
    "React with :joy:",
    "Open reactions",
    "Reply in thread",
    "Quote",
    "Copy link",
    "More actions",
  ]);

  const pickerButton = actionBar.getByRole("button", {
    name: "Open reactions",
  });
  await expect(pickerButton.locator("svg")).toHaveClass(/lucide-smile-plus/);

  const divider = actionBar.getByTestId("message-action-divider");
  await expect(divider).toBeVisible();
  const [pickerBox, dividerBox, replyBox] = await Promise.all([
    actionBar.getByRole("button", { name: "Open reactions" }).boundingBox(),
    divider.boundingBox(),
    actionBar.getByRole("button", { name: "Reply in thread" }).boundingBox(),
  ]);
  expect(pickerBox).not.toBeNull();
  expect(dividerBox).not.toBeNull();
  expect(replyBox).not.toBeNull();
  if (!pickerBox || !dividerBox || !replyBox) {
    throw new Error("Message action order bounds missing.");
  }
  expect(pickerBox.x + pickerBox.width).toBeLessThan(dividerBox.x);
  expect(dividerBox.x + dividerBox.width).toBeLessThan(replyBox.x);

  const copyLink = actionBar.getByTestId(`copy-link-message-${replyId}`);
  await expect(copyLink).toHaveAccessibleName("Copy link");
  await copyLink.hover();
  await expect(page.getByRole("tooltip", { name: "Copy link" })).toBeVisible();

  const expectedLink = `kuzz://message?channel=${GENERAL_CHANNEL_ID}&id=${replyId}&thread=${rootId}`;
  await copyLink.click();
  await expect
    .poll(async () => (await latestClipboardWrite(page))?.payload.text)
    .toBe(expectedLink);
  await expect(
    page.locator("[data-sonner-toast]").filter({
      hasText: "Link copied to clipboard",
    }),
  ).toBeVisible();

  await actionBar.getByTestId(`more-actions-${replyId}`).click();
  await page.getByTestId(`copy-message-link-${replyId}`).click();
  await expect
    .poll(async () => {
      const writes = (await page.evaluate(() =>
        (window.__KUZZ_E2E_COMMAND_LOG__ ?? []).filter(
          ({ command }) => command === "copy_text_to_clipboard",
        ),
      )) as Array<{ payload: unknown }>;
      return writes.map(({ payload }) => payload);
    })
    .toEqual([{ text: expectedLink }, { text: expectedLink }]);

  const [barBox, panelBox] = await Promise.all([
    actionBar.boundingBox(),
    threadPanel.boundingBox(),
  ]);
  expect(barBox).not.toBeNull();
  expect(panelBox).not.toBeNull();
  if (!barBox || !panelBox) throw new Error("Message action bounds missing.");
  expect(barBox.x).toBeGreaterThanOrEqual(panelBox.x);
  expect(barBox.x + barBox.width).toBeLessThanOrEqual(
    panelBox.x + panelBox.width,
  );
});

test("pending and huddle rows omit both copy-link surfaces", async ({
  page,
}) => {
  await page.goto("/");
  await page.getByTestId("channel-general").click();
  await expect(page.getByTestId("chat-title")).toHaveText("general");
  await expect
    .poll(() =>
      page.evaluate(
        () => typeof window.__KUZZ_E2E_EMIT_MOCK_MESSAGE__ === "function",
      ),
    )
    .toBe(true);

  const { huddleId, pendingId } = await page.evaluate((huddleKind) => {
    const emit = window.__KUZZ_E2E_EMIT_MOCK_MESSAGE__;
    if (!emit) throw new Error("Mock message emitter is unavailable.");
    const pending = emit({
      channelName: "general",
      content: "Pending copy-link regression",
      id: "c".repeat(64),
      pending: true,
    });
    const huddle = emit({
      channelName: "general",
      content: JSON.stringify({
        ephemeral_channel_id: "10000000-0000-4000-8000-000000000001",
      }),
      id: "d".repeat(64),
      kind: huddleKind,
    });
    return { huddleId: huddle.id, pendingId: pending.id };
  }, KIND_HUDDLE_STARTED);

  await expectCopyLinkUnavailable(
    page.locator(`[data-message-id="${pendingId}"]`),
    pendingId,
  );
  await expectCopyLinkUnavailable(
    page.locator(`[data-message-id="${huddleId}"]`),
    huddleId,
  );
});

test("keyboard: Tab and Shift+Tab reach a row's actions before any hover", async ({
  page,
}) => {
  await installMockBridge(page);
  await page.goto("/");
  await page.getByTestId("channel-general").click();
  const rows = page.getByTestId("message-row");
  await expect(rows.last()).toBeVisible();

  const opener = rows.nth(-2).locator('[data-testid^="row-actions-focus-"]');
  await rows.last().locator("button, a").first().focus();
  // Back through the last row's own buttons, then into the row above it.
  for (let i = 0; i < 12 && (await opener.count()) > 0; i += 1) {
    await page.keyboard.press("Shift+Tab");
  }
  await expect(opener).toHaveCount(0);
  await expect(
    rows.nth(-2).locator('[data-testid^="more-actions-"]:focus'),
  ).toHaveCount(1);

  // Coming from above lands on the first button.
  const row = rows.nth(-3);
  await page.evaluate(() => (document.activeElement as HTMLElement).blur());
  await row.locator('[data-testid^="row-actions-focus-"]').focus();
  await expect(
    row.locator('[data-testid^="message-action-bar-"] button:focus'),
  ).toHaveCount(1);
  await expect(row.locator('[data-testid^="more-actions-"]:focus')).toHaveCount(
    0,
  );
});

test("a hover on the thread summary mounts the row's actions", async ({
  page,
}) => {
  await installMockBridge(page);
  await page.goto("/");
  await page.waitForFunction(
    () => typeof window.__KUZZ_E2E_EMIT_MOCK_MESSAGE__ === "function",
  );
  await page.getByTestId("channel-general").click();
  const rootId = await page.evaluate(() => {
    const emit = window.__KUZZ_E2E_EMIT_MOCK_MESSAGE__;
    if (!emit) throw new Error("Mock message emitter is unavailable.");
    return emit({ channelName: "general", content: "Summary root" }).id;
  });
  await expect(page.locator(`[data-message-id="${rootId}"]`)).toBeVisible();
  await page.evaluate((id) => {
    window.__KUZZ_E2E_EMIT_MOCK_MESSAGE__?.({
      channelName: "general",
      content: "Summary reply",
      parentEventId: id,
    });
  }, rootId);
  const summary = page.locator(
    `[data-testid="message-thread-summary"][data-thread-head-id="${rootId}"]`,
  );
  await expect(summary).toBeVisible();
  await expect(page.getByTestId(`message-action-bar-${rootId}`)).toHaveCount(0);
  await summary.hover();
  await expect(page.getByTestId(`message-action-bar-${rootId}`)).toHaveCount(1);
});

test("narrowing the window shows a row's actions without a hover", async ({
  page,
}) => {
  await installMockBridge(page);
  await page.goto("/");
  await page.getByTestId("channel-general").click();
  const row = page.getByTestId("message-row").last();
  const bar = row.locator('[data-testid^="message-action-bar-"]');
  await expect(row).toBeVisible();
  await expect(bar).toHaveCount(0);
  await page.setViewportSize({ height: 800, width: 500 });
  await expect(bar).toHaveCount(1);
});
