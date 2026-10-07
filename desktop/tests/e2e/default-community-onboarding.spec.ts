import { expect, test, type Page } from "@playwright/test";
import { installMockBridge, TEST_IDENTITIES } from "../helpers/bridge";
import { seedActiveIdentity } from "../helpers/onboarding";

const ORIGIN = "https://custody.example:8443";
const RELAY = "wss://custody.example:8443";
const TRANSACTION = "buzz-community-onboarding-transaction.v1";
const PUBKEY = TEST_IDENTITIES.alice.pubkey;

type TestWindow = Window & {
  __TAURI_INTERNALS__: {
    invoke: (command: string, args?: unknown) => Promise<unknown>;
  };
  defaultCommunityTest: {
    commands: string[];
    releaseApply?: () => void;
    failApply: boolean;
  };
};

// Only the native boundary is mocked. The real App, machine setup, transaction,
// persistence, workspace gate and RelayClient AUTH completion all run.
async function installDefaultCommunity(
  page: Page,
  options: {
    existing?: boolean;
    completed?: boolean;
    holdApply?: boolean;
    failApply?: boolean;
    profileExists?: boolean;
    holdLogin?: boolean;
  } = {},
) {
  await seedActiveIdentity(page, { ...TEST_IDENTITIES.alice, username: "" });
  await page.addInitScript(
    ({ origin, holdApply, failApply, holdLogin }) => {
      const w = window as TestWindow;
      const state = {
        commands: [] as string[],
        failApply: !!failApply,
        releaseApply: undefined as (() => void) | undefined,
      };
      w.defaultCommunityTest = state;
      let signedIn = false;
      let pendingLogin: { id: string; reject: (error: Error) => void } | null =
        null;
      let invoke: TestWindow["__TAURI_INTERNALS__"]["invoke"];
      w.__TAURI_INTERNALS__ = {} as TestWindow["__TAURI_INTERNALS__"];
      Object.defineProperty(w.__TAURI_INTERNALS__, "invoke", {
        configurable: true,
        set: (value) => {
          invoke = value;
        },
        get: () => async (command: string, args?: unknown) => {
          state.commands.push(command);
          if (command === "cancel_google_login") {
            const id = (args as { attemptId: string }).attemptId;
            if (pendingLogin?.id !== id) return false;
            pendingLogin.reject(new Error("Google sign-in cancelled."));
            pendingLogin = null;
            return true;
          }
          if (command === "login_with_google" && holdLogin) {
            const id = (args as { attemptId: string }).attemptId;
            return new Promise((_, reject) => {
              pendingLogin = { id, reject };
              window.__BUZZ_E2E_EMIT_TAURI_EVENT__?.("google-login-progress", {
                attemptId: id,
                phase: "waiting",
              });
            });
          }
          if (command === "login_with_google") signedIn = true;
          if (
            command === "get_token_auth_status" ||
            command === "login_with_google"
          ) {
            return {
              origin,
              supported: true,
              providers: ["google"],
              state: signedIn ? "active" : "signed_out",
              principal: null,
              deviceId: null,
              reason: null,
              keyBackupSupported: true,
              keyBackup: signedIn,
            };
          }
          if (command === "get_ws_auth_frame") return null;
          if (command === "apply_workspace") {
            if (state.failApply) {
              state.failApply = false;
              throw new Error("Apply failed for test");
            }
            if (holdApply) {
              await new Promise<void>((resolve) => {
                state.releaseApply = resolve;
              });
            }
          }
          return invoke(command, args);
        },
      });
    },
    {
      origin: ORIGIN,
      holdLogin: options.holdLogin,
      holdApply: options.holdApply,
      failApply: options.failApply,
    },
  );
  await installMockBridge(
    page,
    {
      profileHasEvent: options.profileExists ?? false,
      acpRuntimesCatalog: [
        {
          id: "claude",
          label: "Claude Code",
          avatar_url: "",
          availability: "available",
          command: "claude",
          binary_path: "/test/claude",
          default_args: [],
          mcp_command: null,
          install_hint: "",
          install_instructions_url: "https://example.com",
          can_auto_install: true,
          underlying_cli_path: null,
          node_required: false,
          auth_status: { status: "logged_in" },
          login_hint: "",
        },
      ],
      globalAgentConfig: {
        env_vars: {},
        provider: null,
        model: null,
        preferred_runtime: null,
      },
    },
    {
      relayWsUrl: RELAY,
      skipCommunitySeed: !options.existing,
      skipOnboardingSeed: !options.completed,
    },
  );
}

async function signIn(page: Page) {
  await page.goto("/");
  await page.getByTestId("onboarding-google-sign-in").click();
  await expect(page.getByTestId("onboarding-page-2")).toBeVisible();
  await expect(page.getByTestId("welcome-setup")).toHaveCount(0);
  expect(
    await page.evaluate(() => localStorage.getItem("buzz-communities")),
  ).toBeNull();
}

async function savedCommunities(page: Page) {
  return page.evaluate(() =>
    JSON.parse(localStorage.getItem("buzz-communities") ?? "[]"),
  );
}

test("Google setup-later uses the same key and waits for backend apply before AUTH/profile", async ({
  page,
}) => {
  await installDefaultCommunity(page, { holdApply: true });
  await signIn(page);
  await page.getByTestId("onboarding-setup-skip").click();
  await expect
    .poll(() =>
      page.evaluate(
        () => !!(window as TestWindow).defaultCommunityTest.releaseApply,
      ),
    )
    .toBe(true);
  await expect(page.getByTestId("community-onboarding-flow")).toBeVisible();
  await expect(page.getByTestId("community-profile-next")).toHaveCount(0);
  expect(
    await page.evaluate(() =>
      (window as TestWindow).defaultCommunityTest.commands.includes(
        "plugin:websocket|connect",
      ),
    ),
  ).toBe(false);
  await page.evaluate(() =>
    (window as TestWindow).defaultCommunityTest.releaseApply?.(),
  );
  await expect(page.getByTestId("community-profile-next")).toBeVisible();
  await expect(page.getByTestId("welcome-setup")).toHaveCount(0);
  const communities = await savedCommunities(page);
  expect(communities).toHaveLength(1);
  expect(communities[0]).toMatchObject({ relayUrl: RELAY, pubkey: PUBKEY });
  const commands = await page.evaluate(
    () => (window as TestWindow).defaultCommunityTest.commands,
  );
  expect(commands).toContain("create_auth_event");
  expect(commands).not.toContain("import_identity");
  expect(commands).not.toContain("persist_current_identity");
});

for (const skip of [false, true]) {
  test(`provider/model setup ${skip ? "skip" : "save"} completes before automatic connection`, async ({
    page,
  }) => {
    await installDefaultCommunity(page, { profileExists: true });
    await signIn(page);
    await page.getByTestId("onboarding-harness-method-subscription").click();
    await page.getByTestId("onboarding-runtime-details-claude").click();
    await expect(page.getByTestId("onboarding-page-config")).toBeVisible();
    expect(await savedCommunities(page)).toHaveLength(0);
    await page
      .getByTestId(skip ? "onboarding-config-skip" : "onboarding-finish")
      .click();
    await expect.poll(() => savedCommunities(page)).toHaveLength(1);
    await expect
      .poll(() =>
        page.evaluate((key) => localStorage.getItem(key), TRANSACTION),
      )
      .toBeNull();
    await expect(page.getByTestId("welcome-setup")).toHaveCount(0);
    await expect(page.getByTestId("community-onboarding-flow")).toHaveCount(0);
    const commands = await page.evaluate(
      () => (window as TestWindow).defaultCommunityTest.commands,
    );
    expect(commands.includes("set_global_agent_config")).toBe(!skip);
    expect(commands).toContain("create_auth_event");
    await page.reload();
    await expect.poll(() => savedCommunities(page)).toHaveLength(1);
    await expect(page.getByTestId("machine-onboarding-gate")).toHaveCount(0);
  });
}

test("denied AUTH remains durable across reload, retries once with the same community and signer", async ({
  page,
}) => {
  await installDefaultCommunity(page);
  await signIn(page);
  await page.evaluate(() =>
    window.__BUZZ_E2E_QUEUE_AUTH_RESPONSES__?.([
      { success: false, message: "restricted: not a relay member" },
    ]),
  );
  await page.getByTestId("onboarding-setup-skip").click();
  await expect(page.getByRole("alert")).toContainText("not a relay member");
  await expect(page.getByTestId("community-profile-next")).toHaveCount(0);
  const before = await savedCommunities(page);
  await page.reload();
  await expect(page.getByRole("alert")).toContainText("not a relay member");
  expect(
    await page.evaluate(() =>
      (window as TestWindow).defaultCommunityTest.commands.includes(
        "create_auth_event",
      ),
    ),
  ).toBe(false);
  await page.getByRole("button", { name: "Retry", exact: true }).click();
  await expect(page.getByTestId("community-profile-next")).toBeVisible();
  expect(await savedCommunities(page)).toEqual(before);
  expect(before[0].pubkey).toBe(PUBKEY);
});

test("backend apply failure is retryable without duplicating the community", async ({
  page,
}) => {
  await installDefaultCommunity(page, { failApply: true });
  await signIn(page);
  await page.getByTestId("onboarding-setup-skip").click();
  await expect(page.getByRole("alert")).toContainText("Apply failed for test");
  const before = await savedCommunities(page);
  await page.getByRole("button", { name: "Retry", exact: true }).click();
  await expect(page.getByTestId("community-profile-next")).toBeVisible();
  expect(await savedCommunities(page)).toEqual(before);
});

test("advanced recovery cancels a pending default connection and ignores its late apply", async ({
  page,
}) => {
  await installDefaultCommunity(page, { holdApply: true });
  await signIn(page);
  await page.getByTestId("onboarding-setup-skip").click();
  await expect
    .poll(() =>
      page.evaluate(
        () => !!(window as TestWindow).defaultCommunityTest.releaseApply,
      ),
    )
    .toBe(true);
  await page
    .getByRole("button", { name: "Advanced setup", exact: true })
    .click();
  await page.evaluate(() =>
    (window as TestWindow).defaultCommunityTest.releaseApply?.(),
  );
  await expect(page.getByTestId("welcome-setup")).toBeVisible();
  expect(await savedCommunities(page)).toHaveLength(0);
  expect(
    await page.evaluate((key) => localStorage.getItem(key), TRANSACTION),
  ).toBeNull();
});

test("configured users keep their chosen community", async ({ page }) => {
  await installDefaultCommunity(page, {
    existing: true,
    completed: true,
    profileExists: true,
  });
  await page.addInitScript(() => {
    const communities = JSON.parse(
      localStorage.getItem("buzz-communities") ?? "[]",
    );
    communities[0].relayUrl = "wss://chosen.example";
    localStorage.setItem("buzz-communities", JSON.stringify(communities));
  });
  await page.goto("/");
  await expect
    .poll(() =>
      page.evaluate(() =>
        (window as TestWindow).defaultCommunityTest.commands.includes(
          "apply_workspace",
        ),
      ),
    )
    .toBe(true);
  expect((await savedCommunities(page))[0].relayUrl).toBe(
    "wss://chosen.example",
  );
  await expect(page.getByTestId("default-community-setup")).toHaveCount(0);
  expect(
    await page.evaluate((key) => localStorage.getItem(key), TRANSACTION),
  ).toBeNull();
});

test("closed login browser can be cancelled and retried without restarting onboarding", async ({
  page,
}) => {
  await installDefaultCommunity(page, { holdLogin: true });
  await page.goto("/");
  const signIn = page.getByTestId("onboarding-google-sign-in");
  await signIn.click();
  await expect(
    page.getByRole("button", { name: "Cancel sign-in", exact: true }),
  ).toBeVisible();
  await expect(signIn).toBeDisabled();
  await page
    .getByRole("button", { name: "Cancel sign-in", exact: true })
    .click();
  await expect(signIn).toBeEnabled();
  await expect(page.getByRole("alert")).toHaveCount(0);
  await signIn.click();
  await expect(
    page.getByRole("button", { name: "Cancel sign-in", exact: true }),
  ).toBeVisible();
  expect(
    await page.evaluate(
      () =>
        (window as TestWindow).defaultCommunityTest.commands.filter(
          (c) => c === "login_with_google",
        ).length,
    ),
  ).toBe(2);
  expect(await savedCommunities(page)).toHaveLength(0);
});
