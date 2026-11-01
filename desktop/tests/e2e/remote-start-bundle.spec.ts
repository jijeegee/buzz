import { expect, type Page, test } from "@playwright/test";

import { installMockBridge, TEST_IDENTITIES } from "../helpers/bridge";

// The owner's `!start` from another device, heard on the room bundle: no
// REQ of its own, listed and hidden agent DMs alike, one start per command.

// e2eBridge mock DM between the owner and charlie.
const AGENT_DM = "DM";
const AGENT_DM_ID = "d1ec7000-d000-4000-8000-000000000001";
const AGENT = TEST_IDENTITIES.charlie.pubkey;
// The mock-mode signed-in identity (e2eBridge DEFAULT_MOCK_IDENTITY).
const OWNER = "deadbeef".repeat(8);

function stoppedAgent() {
  return {
    pubkey: AGENT,
    name: "Remote agent",
    status: "stopped" as const,
    channelNames: ["agents"],
  };
}

async function startCount(page: Page) {
  return page.evaluate(
    () =>
      (window.__KUZZ_E2E_COMMANDS__ ?? []).filter(
        (command) => command === "start_managed_agent",
      ).length,
  );
}

/** Whether a live bundle chunk carries the agent DM. */
async function agentDmIsLive(page: Page) {
  return page.evaluate((roomId) => {
    const open = window.__KUZZ_E2E_LIVE_REQS__?.() ?? [];
    return open.some(
      (sub) =>
        sub.subIdClass === "live" &&
        sub.filters.some((filter) =>
          ((filter as { "#h"?: string[] })["#h"] ?? []).includes(roomId),
        ),
    );
  }, AGENT_DM_ID);
}

/** REQs shaped like the old remote-start listener's (owner's kind 9). */
async function remoteStartReqs(page: Page) {
  return page.evaluate(
    () =>
      (window.__KUZZ_E2E_REQ_LOG__?.() ?? []).filter(
        (entry) =>
          entry.type === "REQ" &&
          entry.filters.some((filter) => {
            const f = filter as { kinds?: number[]; authors?: string[] };
            return (
              (f.authors?.length ?? 0) > 0 &&
              f.kinds?.length === 1 &&
              f.kinds[0] === 9
            );
          }),
      ).length,
  );
}

async function sendStart(page: Page) {
  await page.evaluate(
    ({ channelName, pubkey, agent }) =>
      window.__KUZZ_E2E_EMIT_MOCK_MESSAGE__?.({
        channelName,
        content: "!start",
        pubkey,
        mentionPubkeys: [agent],
      }),
    { channelName: AGENT_DM, pubkey: OWNER, agent: AGENT },
  );
}

for (const hidden of [false, true]) {
  test(`a !start in a ${hidden ? "hidden" : "listed"} agent DM starts the agent once, with no REQ`, async ({
    page,
  }) => {
    await installMockBridge(page, {
      managedAgents: [stoppedAgent()],
      ...(hidden ? { hiddenDms: [AGENT_DM] } : {}),
    });
    await page.goto("/");
    await expect(page.getByTestId("channel-general")).toBeVisible();
    await expect.poll(() => agentDmIsLive(page)).toBe(true);
    // Let the agents and channels load (the watch's baseline).
    await page.waitForTimeout(2_000);

    await sendStart(page);
    // Started after the 1.5 s settle, then never again.
    await expect.poll(() => startCount(page), { timeout: 10_000 }).toBe(1);
    await page.waitForTimeout(2_500);
    expect(await startCount(page)).toBe(1);
    expect(await remoteStartReqs(page)).toBe(0);
  });
}
