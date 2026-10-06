import { expect, test } from "@playwright/test";
import { installMockBridge } from "../helpers/bridge";
import { waitForAnimations } from "../helpers/animations";

test("shared thread names support limits, Korean composition, live updates and clearing", async ({
  page,
}) => {
  test.setTimeout(90_000);
  await installMockBridge(page);
  await page.goto("/");
  await page.getByTestId("channel-general").click();
  await expect(page.getByTestId("chat-title")).toHaveText("general");
  const root = await page.evaluate(() =>
    window.__BUZZ_E2E_EMIT_MOCK_MESSAGE__?.({
      channelName: "general",
      content: "Thread naming task",
    }),
  );
  if (!root) throw new Error("No thread head");
  await page.evaluate(
    (id) =>
      window.__BUZZ_E2E_EMIT_MOCK_MESSAGE__?.({
        channelName: "general",
        content: "Working on it",
        parentEventId: id,
      }),
    root.id,
  );
  // Reopen after seeding: the conversation bridge can own timeline delivery
  // without registering a legacy relayClient subscription.
  await page.getByTestId("channel-engineering").click();
  await page.getByTestId("channel-general").click();
  await page.getByTestId(`reply-message-${root.id}`).click({ force: true });
  const panel = page.getByTestId("message-thread-panel");
  await panel.getByRole("button", { name: "Set thread name" }).click();
  const input = page.getByRole("textbox", { name: "Thread name", exact: true });
  const save = page.getByRole("button", { name: "Save", exact: true });
  await input.fill("a".repeat(41));
  await expect(save).toBeDisabled();
  await input.fill("a".repeat(40));
  await expect(save).toBeEnabled();
  await input.fill("가".repeat(21));
  await expect(save).toBeDisabled();
  await input.fill("가".repeat(20));
  await expect(save).toBeEnabled();
  await input.dispatchEvent("keydown", {
    key: "Enter",
    code: "Enter",
    isComposing: true,
  });
  await expect(input).toBeVisible();
  await input.fill("쓰레드 이름 기능");
  await save.click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(
    panel.getByText("쓰레드 이름 기능", { exact: true }),
  ).toBeVisible();
  const summary = page
    .locator(
      `[data-testid="message-thread-summary"][data-thread-head-id="${root.id}"]`,
    )
    .first();
  await expect(summary).toContainText("쓰레드 이름 기능");
  await waitForAnimations(page);
  await panel.screenshot({ path: "test-results/thread-name.png" });
  // A different participant renames the thread over the live subscription.
  await page.evaluate(
    (id) =>
      window.__BUZZ_E2E_EMIT_MOCK_MESSAGE__?.({
        channelName: "general",
        kind: 40009,
        content: "공유된 새 이름",
        createdAt: Math.floor(Date.now() / 1000) + 2,
        extraTags: [["e", id]],
      }),
    root.id,
  );
  await expect(
    panel.getByText("공유된 새 이름", { exact: true }),
  ).toBeVisible();
  await panel.getByRole("button", { name: "Rename thread" }).click();
  await input.fill("");
  await save.click();
  await expect(
    panel.getByRole("button", { name: "Set thread name" }),
  ).toBeVisible();
  await expect(summary).not.toContainText("공유된 새 이름");
});
