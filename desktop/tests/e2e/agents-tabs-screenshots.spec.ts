import { expect, test } from "@playwright/test";

import { waitForAnimations } from "../helpers/animations";
import { installMockBridge, TEST_IDENTITIES } from "../helpers/bridge";

const SHOTS = "test-results/agents-tabs";

const MOCK = {
  activePersonaIds: ["builtin:fizz", "builtin:honey", "builtin:pollen"],
  managedAgents: [
    {
      pubkey: TEST_IDENTITIES.alice.pubkey,
      name: "Fizz",
      personaId: "builtin:fizz",
      status: "running" as const,
    },
    {
      pubkey: TEST_IDENTITIES.bob.pubkey,
      name: "Fizz 2",
      personaId: "builtin:fizz",
      status: "stopped" as const,
    },
    {
      pubkey: TEST_IDENTITIES.tyler.pubkey,
      name: "Opus",
      status: "running" as const,
    },
  ],
  teams: [
    {
      id: "team-welcome",
      name: "Welcome Team",
      personaIds: ["builtin:fizz", "builtin:honey", "builtin:pollen"],
    },
  ],
};

async function gotoAgentsView(page: import("@playwright/test").Page) {
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

test.describe("Agents page tabs", () => {
  test("01-my-agents: real agents only, each with its own card", async ({
    page,
  }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await installMockBridge(page, MOCK);
    await gotoAgentsView(page);

    const myAgents = page.getByTestId("agents-my-agents");
    await expect(
      myAgents.getByTestId(`managed-agent-${TEST_IDENTITIES.alice.pubkey}`),
    ).toBeVisible();
    await expect(
      myAgents.getByTestId(`managed-agent-${TEST_IDENTITIES.bob.pubkey}`),
    ).toBeVisible();
    // Honey and Pollen are presets with no agent: not on this tab.
    await expect(myAgents.getByText("Honey")).toHaveCount(0);
    await expect(page.getByTestId("agents-list")).not.toContainText("Honey");

    await waitForAnimations(page);
    await page.screenshot({ path: `${SHOTS}/01-my-agents.png` });
  });

  test("02-presets: templates with Create agent and team presets", async ({
    page,
  }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await installMockBridge(page, MOCK);
    await gotoAgentsView(page);

    await page.getByTestId("agents-tab-presets").click();
    const presets = page.getByTestId("agents-presets");
    await expect(
      presets.getByTestId("persona-agent-row-builtin:fizz"),
    ).toContainText("used by 2 agents");
    await expect(
      presets.getByTestId("persona-agent-row-builtin:honey"),
    ).toContainText("not used yet");
    await expect(presets.getByTestId("team-card-team-welcome")).toBeVisible();

    await waitForAnimations(page);
    await page.screenshot({ path: `${SHOTS}/02-presets.png` });
  });

  test("03-teams: make a team of real agents and add it to a channel", async ({
    page,
  }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await installMockBridge(page, MOCK);
    await gotoAgentsView(page);

    const myAgents = page.getByTestId("agents-my-agents");
    await myAgents.getByTestId("new-my-team-card").click();
    const dialog = page.getByTestId("my-team-dialog");
    await dialog.getByTestId("my-team-name").fill("Build crew");
    await dialog
      .getByTestId(`my-team-agent-${TEST_IDENTITIES.alice.pubkey}`)
      .check();
    await dialog
      .getByTestId(`my-team-agent-${TEST_IDENTITIES.tyler.pubkey}`)
      .check();
    await waitForAnimations(page);
    await page.screenshot({ path: `${SHOTS}/03-team-dialog.png` });
    await dialog.getByTestId("my-team-save").click();
    await expect(dialog).toHaveCount(0);

    const team = myAgents.getByText("Build crew");
    await expect(team).toBeVisible();
    await team.scrollIntoViewIfNeeded();
    await waitForAnimations(page);
    await page.screenshot({ path: `${SHOTS}/03-teams.png` });

    // Team presets: Set up on…, no Add to channel.
    await page.getByTestId("agents-tab-presets").click();
    await page.getByTestId("team-set-up-team-welcome").click();
    await expect(
      page.getByTestId("team-set-up-here-team-welcome"),
    ).toBeVisible();
    await waitForAnimations(page);
    await page.screenshot({ path: `${SHOTS}/04-team-preset-set-up.png` });
  });
});
