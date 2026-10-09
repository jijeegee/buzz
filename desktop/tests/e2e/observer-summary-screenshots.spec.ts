import { readFileSync } from "node:fs";

import { expect, type Locator, type Page, test } from "@playwright/test";

import { installMockBridge, TEST_IDENTITIES } from "../helpers/bridge";

// Feeds the shared observer-summary fixtures (real buzz-acp captures plus
// synthetic tier cases) through the real activity panel.
const SHOTS =
  process.env.OBSERVER_SUMMARY_SHOTS ?? "test-results/observer-summary";

const AGENT_PUBKEY = TEST_IDENTITIES.tyler.pubkey;
const CHANNEL_ID = "94a444a4-c0a3-5966-ab05-530c6ddc2301"; // #agents
const MANAGED_AGENTS = [
  {
    pubkey: AGENT_PUBKEY,
    name: "Observer Agent",
    status: "running" as const,
    channelNames: ["agents"],
  },
];

type FixtureEvent = {
  seq: number;
  timestamp: string;
  turnId: string | null;
  channelId: string | null;
} & Record<string, unknown>;

// Fixture events re-homed into #agents so the channel-scoped panel shows them.
// The store dedupes by (seq, timestamp), so fixtures seeded together are
// shifted `offset` minutes (and seqs) apart.
function fixtureEvents(name: string, offset = 0): FixtureEvent[] {
  const fixture = JSON.parse(
    readFileSync(
      new URL(
        `../../../test-fixtures/observer-summary/${name}`,
        import.meta.url,
      ),
      "utf8",
    ),
  ) as { events: FixtureEvent[] };
  return fixture.events.map((event) => ({
    ...event,
    channelId: CHANNEL_ID,
    seq: event.seq + offset * 1000,
    timestamp: new Date(
      Date.parse(event.timestamp) + offset * 60_000,
    ).toISOString(),
  }));
}

async function openActivityPanel(page: Page) {
  await page.goto("/", { waitUntil: "domcontentloaded" });
  await page.waitForFunction(
    () => typeof window.__BUZZ_E2E_SEED_OBSERVER_EVENTS__ === "function",
    null,
    { timeout: 10_000 },
  );
  await page.getByTestId("channel-agents").click();
  await expect(page.getByTestId("chat-title")).toHaveText("agents");
  const messageRow = page
    .getByTestId("message-row")
    .filter({ has: page.getByText("Observer Agent", { exact: false }) });
  await expect(messageRow.first()).toBeVisible({ timeout: 8_000 });
  await messageRow.first().getByRole("button").first().click();
  await page.getByTestId(`user-profile-view-activity-${AGENT_PUBKEY}`).click();
  const panel = page.getByTestId("agent-session-thread-panel");
  await expect(panel).toBeVisible({ timeout: 10_000 });
  return panel;
}

async function seed(page: Page, events: FixtureEvent[]) {
  await page.evaluate(
    ({ pubkey, evts }) => {
      window.__BUZZ_E2E_SEED_OBSERVER_EVENTS__?.({
        agentPubkey: pubkey,
        events: evts as never,
      });
    },
    { pubkey: AGENT_PUBKEY, evts: events },
  );
}

async function settle(panel: Locator) {
  await panel.evaluate((el) =>
    Promise.all(
      el
        .getAnimations({ subtree: true })
        .filter(
          (a) => a.effect?.getTiming().iterations !== Number.POSITIVE_INFINITY,
        )
        .map((a) => a.finished),
    ),
  );
}

test.describe("observer summary screenshots", () => {
  test.use({ viewport: { width: 1280, height: 1000 } });

  test("01 — free tier: summary lines, gaps, closed tools, badge", async ({
    page,
  }) => {
    await installMockBridge(page, { managedAgents: MANAGED_AGENTS });
    const panel = await openActivityPanel(page);
    await seed(page, [
      ...fixtureEvents("captured-free.json"),
      ...fixtureEvents("synthetic-gap.json", 1),
      // turn-c runs in another channel and never ends; re-homed into
      // #agents it would read as a tool left running by an ended turn.
      ...fixtureEvents("synthetic-turn-end.json", 2).filter(
        (event) => event.turnId !== "turn-c",
      ),
    ]);

    await expect(panel.getByTestId("observer-summary-badge")).toHaveText(
      "Summary view",
    );
    await expect(
      panel.getByText("12 tools ran in between (Read, Bash, Edit…)"),
    ).toBeVisible();
    await expect(panel.getByText("3 updates skipped")).toBeVisible();
    await settle(panel);
    await panel.screenshot({ path: `${SHOTS}/desktop-01-free-summary.png` });

    // The ended turns' leftover tools: "Run tests" completed, "Build" failed.
    await panel.getByText("2 tool calls").click();
    await expect(
      panel.getByTestId("transcript-tool-item").filter({ hasText: "Build" }),
    ).toBeVisible();
    await settle(panel);
    await panel.screenshot({ path: `${SHOTS}/desktop-01b-turn-end.png` });

    // Expanded summary row: the preview stands in for the parameters and
    // there is no result or placeholder.
    const row = panel
      .getByTestId("transcript-tool-item")
      .filter({ hasText: "big read" });
    await row.locator("summary").click();
    await expect(row.getByText("Parameters")).toBeVisible();
    await expect(row.getByText("Waiting for tool details.")).toHaveCount(0);
    await settle(panel);
    await panel.screenshot({ path: `${SHOTS}/desktop-02-free-expanded.png` });
  });

  test("02 — standard tier: 200-char result through the result path", async ({
    page,
  }) => {
    await installMockBridge(page, { managedAgents: MANAGED_AGENTS });
    const panel = await openActivityPanel(page);
    await seed(page, fixtureEvents("synthetic-standard.json"));

    await expect(panel.getByTestId("observer-summary-badge")).toBeVisible();
    const row = panel
      .getByTestId("transcript-tool-item")
      .filter({ hasText: "README" });
    await row.locator("summary").click();
    await expect(row.getByText("Result")).toBeVisible();
    await settle(panel);
    await panel.screenshot({ path: `${SHOTS}/desktop-03-standard.png` });
  });

  test("03 — premium tier renders as before, without the badge", async ({
    page,
  }) => {
    await installMockBridge(page, { managedAgents: MANAGED_AGENTS });
    const panel = await openActivityPanel(page);
    await seed(page, fixtureEvents("captured-premium.json"));

    await expect(panel.getByText("small chunk A turn 1")).toBeVisible();
    await expect(panel.getByTestId("observer-summary-badge")).toHaveCount(0);
    await settle(panel);
    await panel.screenshot({ path: `${SHOTS}/desktop-04-premium.png` });
  });
});
