import { expect, test, type Page } from "@playwright/test";

import { installMockBridge } from "../helpers/bridge";

// Opening a thread from a Chats row whose root is not on the timeline
// (docs/message-sync.md#opening-a-thread-from-its-root-id). This was the
// bounce: the route's fetch found no root and the channel screen closed the
// panel again. The panel must open on the root id alone and stay open, with
// a placeholder head named after the thread, while the relay cannot answer.

const GENERAL = "9a1657ac-f7aa-5db0-b632-d8bbeb6dfb50";
// Not in the mock's channel window, so never among the loaded events.
const ROOT = "0123456789abcdef".repeat(4);
const THREAD_NAME = "Old launch thread";

async function seedChatState(page: Page) {
  await page.addInitScript(
    ({ channelId, rootId, threadName }) => {
      const at = Math.floor(Date.now() / 1000) - 60;
      (
        window as { __KUZZ_E2E_CHAT_STATE__?: unknown }
      ).__KUZZ_E2E_CHAT_STATE__ = {
        rows: [
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
            dm_members: [],
            thread_name: threadName,
            follow_state: "followed",
            closed: false,
            last_seq: null,
            read_seq: null,
            unread: 0,
            unread_overflow: false,
            unread_mentions: 0,
            unread_mentions_overflow: false,
            unread_mark: false,
            latest: null,
            // No root copy to hand over, as for a root the relay hides.
            root: null,
          },
        ],
        threads: [],
      };
    },
    { channelId: GENERAL, rootId: ROOT, threadName: THREAD_NAME },
  );
}

/** Every relay root lookup fails, as when the relay cannot be reached. */
async function failThreadRootLookups(page: Page) {
  await page.evaluate(() => {
    const w = window as typeof window & {
      __TAURI_INTERNALS__: {
        invoke: (
          command: string,
          payload: unknown,
          options: unknown,
        ) => Promise<unknown>;
      };
    };
    const original = w.__TAURI_INTERNALS__.invoke.bind(w.__TAURI_INTERNALS__);
    w.__TAURI_INTERNALS__.invoke = async (command, payload, options) => {
      if (command === "get_thread_root") {
        throw new Error("relay unreachable: request timed out");
      }
      return original(command, payload, options);
    };
  });
}

test("a Chats thread row whose root is not loaded opens the panel and it stays open", async ({
  page,
}) => {
  await seedChatState(page);
  await installMockBridge(page);
  await page.goto(`/#/channels/${GENERAL}`);
  await expect(page.getByTestId("message-timeline")).toBeVisible();
  await failThreadRootLookups(page);
  await expect(
    page.locator(`[data-testid="message-row"][data-message-id="${ROOT}"]`),
  ).toHaveCount(0);

  const row = page.getByTestId(`chat-thread-${ROOT}`);
  await expect(row).toBeVisible();
  await row.click();

  const panel = page.getByTestId("message-thread-panel");
  await expect(panel).toBeVisible();
  await expect(page).toHaveURL(new RegExp(`thread=${ROOT}`));
  await expect(
    panel.getByTestId("message-thread-root-placeholder"),
  ).toContainText(THREAD_NAME);

  // Past the first relay retry (2 s): still open, still on the thread.
  await page.waitForTimeout(3_000);
  await expect(panel).toBeVisible();
  await expect(page).toHaveURL(new RegExp(`thread=${ROOT}`));

  // Closing and opening it again does not reuse a stale route key.
  await page.getByTestId("auxiliary-panel-close").click();
  await expect(panel).toHaveCount(0);
  await row.click();
  await expect(panel).toBeVisible();
  await page.waitForTimeout(1_000);
  await expect(panel).toBeVisible();
  await expect(page).toHaveURL(new RegExp(`thread=${ROOT}`));
});
