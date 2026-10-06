import { expect, test } from "@playwright/test";
import { TEST_IDENTITIES } from "../helpers/bridge";
import { installChatListBridge } from "../helpers/chatListBridge";

const MOCK_PUBKEY = "deadbeef".repeat(8);
const SORT_STORAGE_KEY = `buzz-channel-sort.v1:${MOCK_PUBKEY}:${encodeURIComponent("ws://localhost:3000")}`;

test("mixed chat activity reorders live while personal pins stay first", async ({
  page,
}) => {
  await page.addInitScript((key) => {
    localStorage.setItem(
      key,
      JSON.stringify({
        version: 1,
        groups: { channels: "alpha", dms: "alpha" },
      }),
    );
  }, SORT_STORAGE_KEY);
  await installChatListBridge(page);
  await page.goto("/");
  const list = page.getByTestId("chat-list");
  const names = () =>
    list
      .locator("[data-channel-id]")
      .evaluateAll((rows) =>
        rows.map((row) => row.getAttribute("data-testid")),
      );
  await list.getByTestId("channel-general").click({ button: "right" });
  await page.getByRole("menuitem", { name: "Pin", exact: true }).click();
  for (const [channelName, year] of [
    ["engineering", 3000],
    ["alice-tyler", 3001],
  ] as const) {
    await list.getByTestId(`channel-${channelName}`).click();
    await expect
      .poll(() =>
        page.evaluate(
          (name) =>
            window.__BUZZ_E2E_HAS_MOCK_LIVE_SUBSCRIPTION__?.({
              channelName: name,
              kind: 9,
              exactChannel: true,
            }) ?? false,
          channelName,
        ),
      )
      .toBe(true);
    await list.getByTestId("channel-general").click();
    await page.evaluate(
      ({ name, createdAt, pubkey }) => {
        window.__BUZZ_E2E_EMIT_MOCK_MESSAGE__?.({
          channelName: name,
          createdAt,
          pubkey,
          content: "Isolated chat-list test",
        });
      },
      {
        name: channelName,
        createdAt: Date.parse(`${year}-01-01T00:00:00Z`) / 1000,
        pubkey: TEST_IDENTITIES.alice.pubkey,
      },
    );
    await expect
      .poll(async () => (await names()).slice(0, 2))
      .toEqual(["channel-general", `channel-${channelName}`]);
  }
  await list.getByTestId("channel-general").click({ button: "right" });
  await page.getByRole("menuitem", { name: "Unpin", exact: true }).click();
  await expect
    .poll(async () => (await names()).slice(0, 2))
    .toEqual(["channel-alice-tyler", "channel-engineering"]);
  await page.getByTestId("chat-list-section-label").hover();
  await page.getByTestId("section-actions-chats").click();
  await expect(
    page.getByRole("menuitem", { name: "Sort", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("menuitem", { name: /Browse channels/ }),
  ).toBeVisible();
});

test("forum sort preference remains independent and survives reload", async ({
  page,
}) => {
  await page.addInitScript(
    (key) =>
      localStorage.setItem(
        key,
        JSON.stringify({ version: 1, groups: { forums: "recent" } }),
      ),
    SORT_STORAGE_KEY,
  );
  await installChatListBridge(page);
  await page.goto("/");
  const names = () =>
    page
      .getByTestId("forum-list")
      .locator("[data-channel-id]")
      .evaluateAll((rows) =>
        rows.map((row) => row.getAttribute("data-testid")),
      );
  await expect
    .poll(names)
    .toEqual(["channel-watercooler", "channel-announcements"]);
  await page.reload();
  await expect
    .poll(names)
    .toEqual(["channel-watercooler", "channel-announcements"]);
});
