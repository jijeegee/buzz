import { expect, test, type Page } from "@playwright/test";
import { installMockBridge } from "../helpers/bridge";

// The forum sends like the channel composer: mentions go out as written,
// nothing waits before the send, the own row shows at once, and a failed
// send stays as a row with Retry / Delete.

const OWNER = "deadbeef".repeat(8);
const REMOTE = "ed".repeat(32);
const FORUM = "a27e1ee9-76a6-5bdf-a5d5-1d85610dad11";

async function install(page: Page) {
  await installMockBridge(page, {
    ownerOnlyAccessBuild: true,
    managedAgents: [],
    searchProfiles: [
      {
        pubkey: REMOTE,
        displayName: "RemoteScout",
        ownerPubkey: OWNER,
        isAgent: true,
      },
    ],
    relayAgents: [
      {
        pubkey: REMOTE,
        name: "RemoteScout",
        ownerPubkey: OWNER,
        respondTo: "allowlist",
        respondToAllowlist: [],
        channelNames: [],
      },
    ],
  });
  await page.goto("/");
  await page.getByTestId("channel-watercooler").click();
  await expect(page.getByTestId("chat-title")).toHaveText("watercooler");
}

async function forumSends(page: Page, kind: number) {
  return page.evaluate(
    (kind) =>
      (window.__KUZZ_E2E_COMMAND_LOG__ ?? []).filter(
        (call) =>
          call.command === "send_channel_message" &&
          (call.payload as { kind?: number })?.kind === kind,
      ),
    kind,
  );
}

async function commandCount(page: Page, command: string) {
  return page.evaluate(
    (command) =>
      (window.__KUZZ_E2E_COMMAND_LOG__ ?? []).filter(
        (call) => call.command === command,
      ).length,
    command,
  );
}

// Hold the actual IPC boundary rather than rely on wall-clock sleeps.
type ForumGateWindow = Window & {
  __TAURI_INTERNALS__: {
    invoke: (command: string, payload?: unknown) => Promise<unknown>;
  };
  forumGateEntered?: boolean;
  forumGateDone?: boolean;
  releaseForumGate?: () => void;
};
async function holdSend(page: Page, failure?: string) {
  await page.evaluate(
    ({ failure }) => {
      const state = window as unknown as ForumGateWindow;
      const invoke = state.__TAURI_INTERNALS__.invoke;
      state.forumGateEntered = false;
      state.forumGateDone = false;
      const gate = new Promise<void>((resolve) => {
        state.releaseForumGate = resolve;
      });
      state.__TAURI_INTERNALS__.invoke = async (command, payload) => {
        if (command !== "send_channel_message") return invoke(command, payload);
        state.__TAURI_INTERNALS__.invoke = invoke;
        state.forumGateEntered = true;
        await gate;
        try {
          if (failure) throw new Error(failure);
          return await invoke(command, payload);
        } finally {
          state.forumGateDone = true;
        }
      };
    },
    { failure },
  );
}
async function waitForSendHeld(page: Page) {
  await expect
    .poll(() =>
      page.evaluate(
        () => (window as unknown as ForumGateWindow).forumGateEntered,
      ),
    )
    .toBe(true);
}
async function releaseSend(page: Page) {
  await page.evaluate(() =>
    (window as unknown as ForumGateWindow).releaseForumGate?.(),
  );
  await expect
    .poll(() =>
      page.evaluate(() => (window as unknown as ForumGateWindow).forumGateDone),
    )
    .toBe(true);
}

test("a forum post mentioning an agent outside the group sends as written", async ({
  page,
}) => {
  await install(page);
  await page.getByRole("button", { name: "Start a new post..." }).click();
  await page.getByTestId("message-input").fill("@Remote");
  const row = page.getByTestId(`mention-suggestion-${REMOTE}`);
  await expect(row).toContainText("RemoteScout");
  await row.locator("button").first().click();
  await page.keyboard.type("hello");
  await page.getByTestId("send-message").click();

  await expect.poll(async () => (await forumSends(page, 45001)).length).toBe(1);
  expect((await forumSends(page, 45001))[0].payload).toEqual(
    expect.objectContaining({
      channelId: FORUM,
      mentionPubkeys: [REMOTE],
      content: "@RemoteScout hello",
    }),
  );
  await expect(page.getByRole("alertdialog")).toHaveCount(0);
  expect(await commandCount(page, "add_channel_members")).toBe(0);
  expect(await commandCount(page, "revalidate_relay_agents")).toBe(0);
  const commands = await page.evaluate(
    () => window.__KUZZ_E2E_COMMANDS__ ?? [],
  );
  for (const command of [
    "start_managed_agent",
    "create_managed_agent",
    "attach_managed_agent",
  ]) {
    expect(commands).not.toContain(command);
  }
});

test("an own forum post shows at once with a clock, which goes once sent", async ({
  page,
}) => {
  await install(page);
  await holdSend(page);
  await page.getByRole("button", { name: "Start a new post..." }).click();
  await page.getByTestId("message-input").fill("Instant post");
  await page.getByTestId("send-message").click();
  await waitForSendHeld(page);

  // Drawn while the relay has not answered; the composer is already closed.
  await expect(page.getByText("Instant post", { exact: true })).toBeVisible();
  await expect(page.getByTestId("message-send-pending")).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Start a new post..." }),
  ).toBeVisible();

  await releaseSend(page);
  await expect(page.getByTestId("message-send-pending")).toHaveCount(0);
  await expect(page.getByText("Instant post", { exact: true })).toHaveCount(1);
});

test("a failed forum reply stays as Not sent with Retry, and the composer stays free", async ({
  page,
}) => {
  await install(page);
  await expect
    .poll(() =>
      page.evaluate(() =>
        window.__KUZZ_E2E_HAS_MOCK_LIVE_SUBSCRIPTION__?.({
          channelName: "watercooler",
        }),
      ),
    )
    .toBe(true);
  const root = await page.evaluate(
    () =>
      window.__KUZZ_E2E_EMIT_MOCK_MESSAGE__?.({
        channelName: "watercooler",
        content: "Reply target",
        kind: 45001,
      })?.id,
  );
  expect(root).toBeTruthy();
  await page.evaluate(
    ({ forum, id }) => {
      window.location.hash = `/channels/${forum}/posts/${id}`;
    },
    { forum: FORUM, id: root },
  );
  await expect(page.getByText("Reply target", { exact: true })).toBeVisible();

  const input = page.getByTestId("message-input");
  await holdSend(page, "transport offline");
  await input.fill("Kept reply");
  await page.getByTestId("send-message").click();
  await waitForSendHeld(page);
  await expect(page.getByText("Kept reply", { exact: true })).toBeVisible();
  await expect(page.getByTestId("message-send-pending")).toBeVisible();
  // Not locked while the send is in flight.
  await expect(input).toHaveAttribute("contenteditable", "true");
  await expect(input).toHaveText("");

  await releaseSend(page);
  const status = page.getByTestId("message-send-status");
  await expect(status).toContainText("Not sent");
  await expect(page.getByTestId("message-send-failed")).toBeVisible();
  await expect(input).toHaveText("");

  // The held attempt never reached the mock bridge, so Retry is its first.
  expect(await forumSends(page, 45003)).toHaveLength(0);
  await status.getByRole("button", { name: "Retry" }).click();
  await expect.poll(async () => (await forumSends(page, 45003)).length).toBe(1);
  expect((await forumSends(page, 45003))[0].payload).toEqual(
    expect.objectContaining({
      channelId: FORUM,
      parentEventId: root,
      content: "Kept reply",
    }),
  );
  await expect(status).toHaveCount(0);
});
