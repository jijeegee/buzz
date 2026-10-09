import { expect, test, type Page } from "@playwright/test";

// Search hits carry no thread tags. These cover the placeholder that search
// splices in before the relay copy arrives: it must not open a reply's own
// empty panel, and must not linger in the main timeline as a channel row.

import { installMockBridge } from "../helpers/bridge";

const GENERAL = "9a1657ac-f7aa-5db0-b632-d8bbeb6dfb50";
const ENGINEERING = "1c7e1c02-87bb-5e88-b2da-5a7a9432d0c9";
const REPLY_TEXT = "zebrafish thread reply probe";

test.beforeEach(async ({ page }) => {
  await installMockBridge(page);
});

async function seedReply(page: Page): Promise<string> {
  await page.goto(
    `/#/channels/${GENERAL}?messageId=mock-general-welcome&thread=mock-general-welcome`,
  );
  const composer = page.getByTestId("thread-composer-overlay");
  await expect(composer).toBeVisible();
  await composer.getByTestId("message-input").fill(REPLY_TEXT);
  await page.keyboard.press("Enter");
  const panel = page.getByTestId("message-thread-panel");
  const row = panel
    .locator("[data-message-id]", { hasText: REPLY_TEXT })
    .last();
  await expect(row).toBeVisible();
  await expect
    .poll(async () => (await row.getAttribute("data-message-id")) ?? "")
    .not.toMatch(/^(|pending.*|local.*)$/);
  const id = (await row.getAttribute("data-message-id")) ?? "";
  // Leave and come back so the channel opens on its main timeline.
  await page.getByTestId("channel-engineering").click();
  await expect(page).toHaveURL(new RegExp(`#/channels/${ENGINEERING}`));
  await page.getByTestId("channel-general").click();
  await expect(page).toHaveURL(new RegExp(`#/channels/${GENERAL}$`));
  await expect(page.getByTestId("message-timeline")).toContainText(
    "checking in",
  );
  return id;
}

async function openSearchHit(page: Page, id: string) {
  await page.keyboard.press("ControlOrMeta+f");
  await page.getByTestId("search-dialog-input").fill("zebrafish");
  await page.getByTestId(`search-result-${id}`).click();
}

async function expectInsideThread(page: Page, id: string) {
  await expect(page).toHaveURL(
    new RegExp(`#/channels/${GENERAL}\\?thread=mock-general-welcome$`),
  );
  const panel = page.getByTestId("message-thread-panel");
  await expect(panel.getByTestId("message-thread-head")).toContainText(
    "Welcome to",
  );
  await expect(panel.locator(`[data-message-id="${id}"]`)).toBeVisible();
  await expect(
    page.getByTestId("message-timeline").locator(`[data-message-id="${id}"]`),
  ).toHaveCount(0);
}

test("reply already loaded: search hit opens its thread at the reply", async ({
  page,
}) => {
  const id = await seedReply(page);
  await openSearchHit(page, id);
  await expectInsideThread(page, id);
});

test("relay copy arrives late: placeholder waits, then opens the thread", async ({
  page,
}) => {
  const id = await seedReply(page);
  // Simulate an older reply that the channel feed has not loaded, so the
  // search-hit placeholder is the only copy until the relay answers.
  const removed = await page.evaluate((eventId) => {
    type CachedQuery = { queryKey: unknown[]; state: { data: unknown } };
    const client = (
      window as unknown as {
        __BUZZ_E2E_QUERY_CLIENT__: {
          getQueryCache: () => { getAll: () => CachedQuery[] };
          setQueryData: (key: unknown[], data: unknown) => void;
        };
      }
    ).__BUZZ_E2E_QUERY_CLIENT__;
    const isTarget = (event: unknown) =>
      (event as { id?: string } | null)?.id === eventId;
    let count = 0;
    for (const query of client.getQueryCache().getAll()) {
      const data = query.state.data;
      if (Array.isArray(data) && data.some(isTarget)) {
        client.setQueryData(
          query.queryKey,
          data.filter((event) => !isTarget(event)),
        );
        count += 1;
      }
    }
    return count;
  }, id);
  expect(removed).toBeGreaterThan(0);
  await page.evaluate((eventId) => {
    (
      window as Window & { __BUZZ_E2E_DEFER_GET_EVENT__?: string | null }
    ).__BUZZ_E2E_DEFER_GET_EVENT__ = eventId;
  }, id);
  await openSearchHit(page, id);
  // While only the placeholder exists, no reply panel may open for it.
  await expect(page.getByTestId("chat-title")).toHaveText("general");
  await expect(page).not.toHaveURL(new RegExp(`thread=${id}`));
  const released = await page.evaluate(() =>
    (
      window as Window & { __BUZZ_E2E_RELEASE_GET_EVENT__?: () => number }
    ).__BUZZ_E2E_RELEASE_GET_EVENT__?.(),
  );
  expect(released).toBeGreaterThan(0);
  await expectInsideThread(page, id);
});
