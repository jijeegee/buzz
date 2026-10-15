import { test, type Page } from "@playwright/test";
import { installMockBridge } from "../helpers/bridge";

const OUT = process.env.SHOT_DIR ?? "test-results/brand-shots";
test.use({ viewport: { width: 1280, height: 800 } });

async function googleAuth(page: Page) {
  await page.addInitScript(() => {
    const w = window as unknown as { __TAURI_INTERNALS__: { invoke: (c: string, a?: unknown) => Promise<unknown> } };
    let invoke: typeof w.__TAURI_INTERNALS__.invoke;
    w.__TAURI_INTERNALS__ = {} as typeof w.__TAURI_INTERNALS__;
    Object.defineProperty(w.__TAURI_INTERNALS__, "invoke", {
      configurable: true,
      set: (v) => { invoke = v; },
      get: () => async (c: string, a?: unknown) => {
        if (c === "get_token_auth_status")
          return { supported: true, origin: "http://localhost:3000", state: "signed_out", providers: ["google"], principal: null, deviceId: null, reason: null, keyBackupSupported: true, keyBackup: false };
        return invoke(c, a);
      },
    });
  });
}

test("landing google", async ({ page }) => {
  await googleAuth(page);
  await installMockBridge(page, undefined, { skipCommunitySeed: true, skipOnboardingSeed: true });
  await page.goto("/");
  await page.getByTestId("machine-onboarding-gate").waitFor();
  await page.waitForTimeout(1500);
  await page.screenshot({ path: `${OUT}/landing-google.png` });
});

test("landing then setup step", async ({ page }) => {
  await installMockBridge(page, undefined, { skipCommunitySeed: true, skipOnboardingSeed: true });
  await page.goto("/");
  await page.getByTestId("machine-onboarding-gate").waitFor();
  await page.waitForTimeout(1200);
  await page.screenshot({ path: `${OUT}/landing-keyfallback.png` });
  await page.getByRole("button", { name: "Create a new identity key" }).click();
  await page.waitForTimeout(1500);
  await page.screenshot({ path: `${OUT}/setup-step.png` });
});

for (const scheme of ["light", "dark"] as const) {
  test(`chat ${scheme}`, async ({ page }) => {
    await page.emulateMedia({ colorScheme: scheme });
    await installMockBridge(page);
    await page.goto("/");
    await page.getByTestId("message-input").first().waitFor({ timeout: 20000 }).catch(() => {});
    await page.waitForTimeout(2000);
    await page.screenshot({ path: `${OUT}/chat-${scheme}.png` });
  });
}

for (const scheme of ["light", "dark"] as const) {
  test(`channel ${scheme}`, async ({ page }) => {
    await page.emulateMedia({ colorScheme: scheme });
    await installMockBridge(page);
    await page.goto("/");
    await page.getByRole("link", { name: "general" }).or(page.getByText("general", { exact: true })).first().click();
    await page.waitForTimeout(2500);
    await page.screenshot({ path: `${OUT}/channel-${scheme}.png` });
  });
}
