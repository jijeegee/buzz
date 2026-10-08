import type { Page } from "@playwright/test";

const INBOX_FILTER_STORAGE_KEY = "buzz.desktop.inbox-filter";

/**
 * Chat-message rows in All / Mentions / Threads / Channels + Threads now enter
 * the real chat screen. The inbox's own detail view still serves Needs action
 * (and the other non-chat filters), so specs that exercise that detail seed
 * their items as `needs_action` and open the inbox on that filter.
 */
export async function openInboxOnNeedsAction(page: Page) {
  await page.addInitScript((key) => {
    window.localStorage.setItem(key, "needs_action");
  }, INBOX_FILTER_STORAGE_KEY);
}

type MockFeedWindow = Window & {
  __BUZZ_E2E_EMIT_MOCK_MESSAGE__?: (input: {
    channelName: string;
    content: string;
    id?: string;
  }) => {
    content: string;
    created_at: number;
    id: string;
    kind: number;
    pubkey: string;
    tags: string[][];
  };
  __BUZZ_E2E_PUSH_MOCK_FEED_ITEM__?: (item: {
    category: "needs_action";
    channel_id: string;
    channel_name: string;
    content: string;
    created_at: number;
    id: string;
    kind: number;
    pubkey: string;
    tags: string[][];
  }) => void;
};

const GENERAL_CHANNEL_ID = "9a1657ac-f7aa-5db0-b632-d8bbeb6dfb50";

/** Seeds one Needs action message in #general and opens it in the detail. */
export async function openNeedsActionDetail(page: Page, content: string) {
  await page.waitForFunction(() => {
    const win = window as MockFeedWindow;
    return (
      typeof win.__BUZZ_E2E_EMIT_MOCK_MESSAGE__ === "function" &&
      typeof win.__BUZZ_E2E_PUSH_MOCK_FEED_ITEM__ === "function"
    );
  });
  const id = await page.evaluate(
    ({ channelId, text }) => {
      const win = window as MockFeedWindow;
      const emit = win.__BUZZ_E2E_EMIT_MOCK_MESSAGE__;
      const push = win.__BUZZ_E2E_PUSH_MOCK_FEED_ITEM__;
      if (!emit || !push) throw new Error("Mock bridge helpers missing.");
      const event = emit({ channelName: "general", content: text });
      push({
        category: "needs_action",
        channel_id: channelId,
        channel_name: "general",
        content: event.content,
        created_at: event.created_at,
        id: event.id,
        kind: event.kind,
        pubkey: event.pubkey,
        tags: event.tags,
      });
      return event.id;
    },
    { channelId: GENERAL_CHANNEL_ID, text: content },
  );
  await page.getByTestId(`home-inbox-item-${id}`).click();
}
