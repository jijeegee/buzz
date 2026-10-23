import { expect, test, type Page } from "@playwright/test";

// Search finds threads by name whatever their state. The relay's
// `thread_search` returns closed and quiet threads the Chats list leaves
// out; they are listed as `group › thread` with a "Done" tag when closed.

import { installMockBridge } from "../helpers/bridge";

const GENERAL = "9a1657ac-f7aa-5db0-b632-d8bbeb6dfb50";
const ROOT = "mock-general-welcome";

async function seedChatState(page: Page) {
  await page.addInitScript(
    ({ channelId, rootId }) => {
      const at = Math.floor(Date.now() / 1000) - 30 * 24 * 60 * 60;
      (
        window as { __KUZZ_E2E_CHAT_STATE__?: unknown }
      ).__KUZZ_E2E_CHAT_STATE__ = {
        rows: [],
        threads: [
          {
            type: "thread",
            key: rootId,
            root_id: rootId,
            at,
            at_us: at * 1_000_000,
            channel_id: channelId,
            channel_type: "stream",
            channel_name: "general",
            archived: false,
            group_chat: false,
            member_count: null,
            dm_members: [],
            thread_name: "Quarterly launch review",
            closed: true,
            match: 2,
          },
        ],
      };
    },
    { channelId: GENERAL, rootId: ROOT },
  );
}

async function searchFor(page: Page, text: string) {
  await page.goto(`/#/channels/${GENERAL}`);
  await expect(page.getByTestId("message-timeline")).toBeVisible();
  await page.keyboard.press("ControlOrMeta+k");
  await page.getByTestId("search-dialog-input").fill(text);
}

test("a closed thread is found by a word in its name and marked Done", async ({
  page,
}) => {
  await seedChatState(page);
  await installMockBridge(page);
  await searchFor(page, "LAUNCH");

  const row = page.getByTestId(`search-result-thread-${ROOT}`);
  await expect(row).toBeVisible();
  await expect(row).toContainText("general");
  await expect(row).toContainText("Quarterly launch review");
  await expect(
    page.getByTestId(`search-result-thread-done-${ROOT}`),
  ).toHaveText("Done");

  await row.click();
  await expect(page).toHaveURL(new RegExp(`thread=${ROOT}`));
  await expect(
    page.getByTestId("message-thread-panel").getByTestId("message-thread-head"),
  ).toContainText("Welcome to");
});

test("without server chat state there is no thread name result", async ({
  page,
}) => {
  await installMockBridge(page);
  await searchFor(page, "launch");
  await expect(page.getByTestId("search-dialog-input")).toHaveValue("launch");
  await expect(page.getByTestId(`search-result-thread-${ROOT}`)).toHaveCount(0);
});
