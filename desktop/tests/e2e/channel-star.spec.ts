import { expect, test } from "@playwright/test";
import { installChatListBridge } from "../helpers/chatListBridge";

const MOCK_PUBKEY = "deadbeef".repeat(8);
const ENGINEERING_ID = "1c7e1c02-87bb-5e88-b2da-5a7a9432d0c9";
const STORAGE_KEY = `buzz-channel-stars.v1:${MOCK_PUBKEY}:${encodeURIComponent("ws://localhost:3000")}`;

test("personal Pin/Unpin uses one Chats list and survives reload", async ({
  page,
}) => {
  await installChatListBridge(page);
  await page.goto("/");
  const list = page.getByTestId("chat-list");
  const first = list.locator("[data-channel-id]").first();
  await expect(list.getByTestId("channel-alice-tyler")).toBeVisible();
  await expect(page.getByTestId("starred-list")).toHaveCount(0);
  await expect(page.getByTestId("stream-list")).toHaveCount(0);
  await expect(page.getByTestId("dm-list")).toHaveCount(0);
  await list.getByTestId("channel-engineering").click({ button: "right" });
  await expect(
    page.getByRole("menuitem", { name: "Mark unread", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("menuitem", { name: "Mute channel", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("menuitem", { name: "Leave channel", exact: true }),
  ).toBeVisible();
  await page.getByRole("menuitem", { name: "Pin", exact: true }).click();
  await expect(first).toHaveAttribute("data-channel-id", ENGINEERING_ID);
  await expect(list.getByTestId(`chat-pinned-${ENGINEERING_ID}`)).toBeVisible();
  await page.reload();
  await expect(first).toHaveAttribute("data-channel-id", ENGINEERING_ID);
  await list.getByTestId("channel-engineering").focus();
  await page.keyboard.press("Shift+F10");
  await page.getByRole("menuitem", { name: "Unpin", exact: true }).click();
  await expect(first).not.toHaveAttribute("data-channel-id", ENGINEERING_ID);
  await page.reload();
  await expect(list).toBeVisible();
  await expect(list.getByTestId(`chat-pinned-${ENGINEERING_ID}`)).toHaveCount(
    0,
  );
  expect(
    await page.evaluate(
      ({ key, id }) =>
        JSON.parse(localStorage.getItem(key) ?? "null")?.channels[id]?.starred,
      { key: STORAGE_KEY, id: ENGINEERING_ID },
    ),
  ).toBe(false);
});

test("DM pin keeps DM controls without channel leave", async ({ page }) => {
  await installChatListBridge(page);
  await page.goto("/");
  const list = page.getByTestId("chat-list");
  const dm = list.getByTestId("channel-alice-tyler");
  await dm.click({ button: "right" });
  await expect(
    page.getByRole("menuitem", { name: "Leave channel", exact: true }),
  ).toHaveCount(0);
  await page.getByRole("menuitem", { name: "Pin", exact: true }).click();
  await expect(list.locator("[data-channel-id]").first()).toHaveAttribute(
    "data-testid",
    "channel-alice-tyler",
  );
  await dm.click();
  await expect(page.getByTestId("message-input")).toBeVisible();
  await expect(list.getByTestId("hide-dm-alice-tyler")).toBeAttached();
});
