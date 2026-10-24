import { expect, test, type Page } from "@playwright/test";

import { installMockBridge } from "../helpers/bridge";

// The own message is drawn before anything reaches the backend: every IPC
// call made after Send is held, and the row must already be on screen.

type GateWindow = Window & {
  __TAURI_INTERNALS__: {
    invoke: (command: string, payload?: unknown) => Promise<unknown>;
  };
  heldCommands?: string[];
  releaseAll?: () => void;
};

async function holdAllIpc(page: Page) {
  await page.evaluate(() => {
    const state = window as unknown as GateWindow;
    const invoke = state.__TAURI_INTERNALS__.invoke;
    state.heldCommands = [];
    let release: () => void = () => {};
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    state.releaseAll = () => {
      state.__TAURI_INTERNALS__.invoke = invoke;
      release();
    };
    state.__TAURI_INTERNALS__.invoke = async (command, payload) => {
      state.heldCommands?.push(command);
      await gate;
      return invoke(command, payload);
    };
  });
}

test.beforeEach(async ({ page }) => {
  await installMockBridge(page);
});

for (const withMention of [false, true]) {
  test(`own group message shows before any backend call${withMention ? " with an agent mention" : ""}`, async ({
    page,
  }) => {
    await page.goto("/");
    await page.getByTestId("channel-general").click();
    await expect(page.getByTestId("chat-title")).toHaveText("general");

    const input = page.getByTestId("message-input");
    let content = "Instant hello";
    if (withMention) {
      await input.fill("Ask @alice");
      await input.press(" ");
      await page.keyboard.type("instant hello");
      content = "Ask @alice instant hello";
      await expect(
        input.locator(".agent-mention-highlight", { hasText: "alice" }),
      ).toBeVisible();
    } else {
      await input.fill(content);
    }

    await holdAllIpc(page);
    await page.getByTestId("send-message").click();
    const row = page
      .getByTestId("message-timeline")
      .getByText(withMention ? "instant hello" : content);
    await expect(row.first()).toBeVisible({ timeout: 1000 });
    await expect(input).toHaveText("");
    const held = await page.evaluate(
      () => (window as unknown as GateWindow).heldCommands ?? [],
    );
    test.info().annotations.push({
      type: "held-before-draw",
      description: held.join(","),
    });
    await page.evaluate(() => (window as unknown as GateWindow).releaseAll?.());
    await expect(page.getByTestId("message-send-pending")).toHaveCount(0);
  });
}
