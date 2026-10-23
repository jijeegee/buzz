import { expect, type Page, test } from "@playwright/test";

import { waitForAnimations } from "../helpers/animations";
import { installMockBridge, TEST_IDENTITIES } from "../helpers/bridge";

const SHOTS =
  process.env.KUZZ_C2_SHOTS_DIR ?? "test-results/external-connect-screenshots";
const AGENT = "a1".repeat(32);

/** This computer's agents, as in the mockup. */
const LOCAL_AGENTS = {
  managedAgents: [
    {
      pubkey: TEST_IDENTITIES.alice.pubkey,
      name: "Fizz",
      status: "running" as const,
    },
    {
      pubkey: TEST_IDENTITIES.bob.pubkey,
      name: "Fizz 2",
      status: "stopped" as const,
    },
    {
      pubkey: TEST_IDENTITIES.tyler.pubkey,
      name: "Opus",
      status: "running" as const,
    },
  ],
};

/**
 * Token sign-in, devices, pairing and Hermes answers at the native boundary,
 * plus a hook to push a relay "new sign-in" notice on its live subscription.
 */
async function installExternalConnectBridge(page: Page) {
  await page.addInitScript(
    ({ agent }) => {
      type Invoke = (command: string, args?: unknown) => Promise<unknown>;
      type Handler = (message: unknown) => void;
      const state = {
        pairingState: "waiting" as "waiting" | "connected",
        hermesFound: true,
        devices: [
          {
            id: "me",
            name: "akak-PC",
            platform: "windows",
            lastSeenAt: null,
            current: true,
          },
        ] as Array<Record<string, unknown>>,
        expiresAt: "",
      };
      const sockets = new Map<number, Handler>();
      const signInSubs: Array<{ id: number; sub: string }> = [];
      const w = window as unknown as {
        __TAURI_INTERNALS__: { invoke: Invoke };
        __C2__: typeof state & { notify: (device: string) => void };
      };
      w.__C2__ = Object.assign(state, {
        notify(device: string) {
          for (const { id, sub } of signInSubs) {
            sockets.get(id)?.({
              type: "Text",
              data: JSON.stringify([
                "EVENT",
                sub,
                {
                  id: `${Date.now()}`.padStart(64, "0"),
                  pubkey: "f".repeat(64),
                  created_at: Math.floor(Date.now() / 1000),
                  kind: 24211,
                  tags: [["p", (w as unknown as { __ME__: string }).__ME__]],
                  content: `New sign-in: ${device}`,
                  sig: "0".repeat(128),
                },
              ]),
            });
          }
        },
      });
      const pairing = () => ({
        id: "p1",
        state: state.pairingState,
        agentPubkey: state.pairingState === "connected" ? agent : null,
        deviceName: state.pairingState === "connected" ? "office-laptop" : null,
        deviceId: state.pairingState === "connected" ? "ext-1" : null,
        createdAt: new Date().toISOString(),
        claimedAt:
          state.pairingState === "connected" ? new Date().toISOString() : null,
        expiresAt: state.expiresAt,
      });

      let invoke: Invoke;
      w.__TAURI_INTERNALS__ = {} as typeof w.__TAURI_INTERNALS__;
      Object.defineProperty(w.__TAURI_INTERNALS__, "invoke", {
        configurable: true,
        set: (value) => {
          invoke = value;
        },
        get: () => async (command: string, args?: unknown) => {
          switch (command) {
            case "get_ws_auth_frame":
              return null;
            case "is_shared_identity":
              return false;
            case "get_token_auth_status":
              return {
                supported: true,
                origin: "http://localhost:3000",
                state: "active",
                providers: ["google"],
                principal: "akak@example.com",
                deviceId: "me",
                reason: null,
                keyBackupSupported: true,
                keyBackup: true,
              };
            case "list_devices":
              return state.devices;
            case "revoke_device":
            case "disconnect_external_device": {
              const { deviceId } = args as { deviceId: string };
              state.devices = state.devices.filter((d) => d.id !== deviceId);
              return null;
            }
            case "start_agent_pairing":
              state.pairingState = "waiting";
              state.expiresAt = new Date(Date.now() + 582_000).toISOString();
              return {
                pairingId: "p1",
                command:
                  "kuzz pair --relay wss://gateway.kuzz.app 7QKF-M2XD-9RTB-H4WN-C8PJ-ZL6V-3EYA",
                expiresAt: state.expiresAt,
                expiresIn: 582,
              };
            case "list_agent_pairings":
              return state.expiresAt ? [pairing()] : [];
            case "get_hermes_status":
              return {
                found: state.hermesFound,
                canConnect: state.hermesFound,
                agents: [],
              };
            case "get_user_profile": {
              // The external agent's own kind:0 profile.
              const { pubkey } = (args ?? {}) as { pubkey?: string };
              if (pubkey?.toLowerCase() === agent) {
                return {
                  pubkey: agent,
                  display_name: "Hermes",
                  avatar_url: null,
                  about: null,
                  nip05_handle: null,
                  owner_pubkey: null,
                  has_profile_event: true,
                };
              }
              return invoke(command, args);
            }
            case "get_presence": {
              const { pubkeys } = args as { pubkeys: string[] };
              const others = pubkeys.filter((p) => p.toLowerCase() !== agent);
              const base = (
                others.length > 0
                  ? await invoke(command, {
                      ...(args as object),
                      pubkeys: others,
                    })
                  : {}
              ) as Record<string, string>;
              return pubkeys.some((p) => p.toLowerCase() === agent)
                ? { ...base, [agent]: "online" }
                : base;
            }
            case "get_identity": {
              const identity = (await invoke(command, args)) as {
                pubkey: string;
              };
              (w as unknown as { __ME__: string }).__ME__ = identity.pubkey;
              return identity;
            }
            case "plugin:websocket|connect": {
              const payload = args as { onMessage: unknown };
              const id = (await invoke(command, args)) as number;
              const raw = payload.onMessage as
                | Handler
                | { onmessage?: Handler };
              const deliver =
                typeof raw === "function" ? raw : raw?.onmessage?.bind(raw);
              if (deliver) sockets.set(id, (m) => deliver([m]));
              return id;
            }
            case "plugin:websocket|send": {
              const { id, message } = args as {
                id: number;
                message?: { data?: string };
              };
              try {
                const frame = JSON.parse(message?.data ?? "null");
                if (
                  Array.isArray(frame) &&
                  frame[0] === "REQ" &&
                  frame
                    .slice(2)
                    .some((f: { kinds?: number[] }) =>
                      f?.kinds?.includes(24211),
                    )
                ) {
                  signInSubs.push({ id, sub: frame[1] });
                }
              } catch {
                // Not JSON: leave it to the mock relay.
              }
              return invoke(command, args);
            }
            default:
              return invoke(command, args);
          }
        },
      });
    },
    { agent: AGENT },
  );
  await installMockBridge(page, LOCAL_AGENTS);
}

async function gotoAgents(page: Page) {
  await page.goto("/", { waitUntil: "domcontentloaded" });
  await expect(page.getByTestId("open-agents-view")).toBeVisible({
    timeout: 10_000,
  });
  await page.getByTestId("open-agents-view").click();
  await page.getByTestId("agents-list-all").click();
  await expect(page.getByTestId("agents-my-agents")).toBeVisible({
    timeout: 10_000,
  });
}

test.describe("Connect external agent (C2)", () => {
  test.beforeEach(async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await installExternalConnectBridge(page);
  });

  test("1a code, connected notice, 1d list and disconnect", async ({
    page,
  }) => {
    await gotoAgents(page);
    await page.evaluate(() => {
      (
        window as unknown as { __C2__: { hermesFound: boolean } }
      ).__C2__.hermesFound = false;
    });

    await page.getByTestId("agents-connect-external").click();
    const dialog = page.getByTestId("connect-external-agent-dialog");
    await expect(
      dialog.getByTestId("connect-external-agent-command"),
    ).toHaveText(
      "kuzz pair --relay wss://gateway.kuzz.app 7QKF-M2XD-9RTB-H4WN-C8PJ-ZL6V-3EYA",
    );
    await expect(
      dialog.getByTestId("connect-external-agent-countdown"),
    ).toContainText(/Expires in 9:[34]\d/);
    await waitForAnimations(page);
    await page.screenshot({ path: `${SHOTS}/1a_connect_code.png` });

    // The other computer claims the code: connected at the next check.
    await page.evaluate(() => {
      const c2 = (
        window as unknown as {
          __C2__: {
            pairingState: string;
            devices: Array<Record<string, unknown>>;
          };
        }
      ).__C2__;
      c2.pairingState = "connected";
      c2.devices.push({
        id: "ext-1",
        name: "office-laptop",
        platform: "external",
        lastSeenAt: null,
        current: false,
      });
    });
    await expect(
      dialog.getByTestId("connect-external-agent-connected"),
    ).toContainText("on office-laptop connected", { timeout: 5_000 });
    await waitForAnimations(page);
    await page.screenshot({ path: `${SHOTS}/1b_1c_connected_code_used.png` });

    await dialog.getByRole("button", { name: "Done" }).click();
    const group = page.getByTestId("agents-external-device-ext-1");
    await expect(group).toContainText("office-laptop");
    await expect(group).toContainText("1 connected agent");
    await expect(group).toContainText("Hermes");
    await expect(
      group.getByTestId(`external-agent-${AGENT}-status`),
    ).toHaveText("Running");
    // The left list has the paired computer too.
    const listSection = page.getByTestId("agents-external-ext-1");
    await expect(listSection).toContainText("office-laptop");
    await expect(listSection).toContainText("Hermes");
    await waitForAnimations(page);
    await page.screenshot({ path: `${SHOTS}/1d_connected.png` });

    await group.getByTestId(`external-agent-add-to-channel-${AGENT}`).click();
    await expect(page.getByRole("dialog")).toContainText("Hermes");
    await waitForAnimations(page);
    await page.screenshot({ path: `${SHOTS}/1d_add_to_channel.png` });
    await page.keyboard.press("Escape");

    await group.getByTestId(`external-agent-disconnect-${AGENT}`).click();
    await expect(
      page.getByTestId("external-agent-disconnect-dialog"),
    ).toBeVisible();
    await waitForAnimations(page);
    await page.screenshot({ path: `${SHOTS}/1e_disconnect_confirm.png` });
    await page.getByTestId("external-agent-disconnect-confirm").click();
    await expect(group).toHaveCount(0);
  });

  test("2 Hermes found on this computer", async ({ page }) => {
    await gotoAgents(page);
    await expect(page.getByTestId("hermes-found-card")).toBeVisible();
    await waitForAnimations(page);
    await page.screenshot({ path: `${SHOTS}/2_hermes_found.png` });
  });

  test("3a new sign-in notice, 3b device list marks it New", async ({
    page,
  }) => {
    await page.goto("/", { waitUntil: "domcontentloaded" });
    await expect(page.getByTestId("open-agents-view")).toBeVisible({
      timeout: 10_000,
    });
    await page.evaluate(() => {
      (
        window as unknown as {
          __C2__: { devices: Array<Record<string, unknown>> };
        }
      ).__C2__.devices.push(
        {
          id: "mac",
          name: "mac-mini",
          platform: "macos",
          lastSeenAt: null,
          current: false,
        },
        {
          id: "s24",
          name: "Galaxy S24",
          platform: "android",
          lastSeenAt: new Date().toISOString(),
          current: false,
        },
      );
    });
    await expect
      .poll(
        () =>
          page.evaluate(() => {
            (
              window as unknown as { __C2__: { notify: (d: string) => void } }
            ).__C2__.notify("Galaxy S24");
            return (
              document.querySelector('[data-testid="new-signin-notice"]') !==
              null
            );
          }),
        { timeout: 10_000 },
      )
      .toBe(true);
    await waitForAnimations(page);
    await page.screenshot({ path: `${SHOTS}/3a_new_signin_notice.png` });

    await page.getByTestId("new-signin-notice-action").click();
    const row = page.locator("[data-new-sign-in]");
    await expect(row).toContainText("Galaxy S24");
    await expect(row.getByTestId("device-new-badge")).toHaveText("New");
    await row.scrollIntoViewIfNeeded();
    await waitForAnimations(page);
    await page.screenshot({ path: `${SHOTS}/3b_devices_after_link.png` });
  });
});
