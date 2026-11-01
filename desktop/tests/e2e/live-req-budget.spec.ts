import { readFileSync, writeFileSync } from "node:fs";
import { expect, type Page, type TestInfo, test } from "@playwright/test";
import { installMockBridge } from "../helpers/bridge";

// Live REQ budget (stage A0 of the subscription consolidation plan).
//
// Counts open live subscriptions on the mock relay socket at fixed screens and
// compares them with test-fixtures/live-req-budget.json ("desktop" key). The
// open set comes from the bridge's REQ/CLOSE/CLOSED wire log, not from the
// mock's stored subscriptions. Counts describe the preview-on configuration:
// installMockBridge opts every preview feature in.
//
// Update mode: KUZZ_UPDATE_REQ_BUDGET=1 settles on a quiet wire instead of the
// fixture and rewrites only the "desktop" key from the measured run. Launch
// ceilings only ratchet up in update mode (max of fixture and run), so run it
// with --repeat-each=5; zero the "launch" block first to lower them.

const FIXTURE_URL = new URL(
  "../../../test-fixtures/live-req-budget.json",
  import.meta.url,
);
const UPDATE = process.env.KUZZ_UPDATE_REQ_BUDGET === "1";
const HOLD_MS = 3_000;
const SETTLE_TIMEOUT_MS = 25_000;
const ROWS_VIEWPORT = { width: 1440, height: 2400 };
const ROOM_A = "random";
const ROOM_B = "engineering";
// Same room type as A, measured in a fresh context so it starts empty.
const ROOM_C = ROOM_A;
const DM = "alice-tyler";
const SUMMARY_IN_TIMELINE =
  '[data-testid="message-timeline"] [data-testid="message-thread-summary"]';

type CheckpointName = "idle" | "roomA" | "roomB" | "threadOpen" | "dmOpen";
type Checkpoint = { live: number; signatures: string[] };
type RowsCheckpoint = Checkpoint & { mountedRows: number };
type DesktopBudget = {
  launch: { oneShot: number; liveSends: number };
  checkpoints: Record<CheckpointName, Checkpoint>;
  threadRows: {
    rows10: RowsCheckpoint;
    livePerMountedRow: number;
    perRowSignatures: string[];
  };
};
type Fixture = {
  description: string;
  desktop: Partial<DesktopBudget> & Record<string, unknown>;
  mobile: Record<string, unknown>;
  target: Record<string, unknown>;
};

type Snapshot = {
  live: { subId: string; signature: string; filters: unknown[] }[];
  other: { subId: string; subIdClass: string; signature: string }[];
  mountedRows: number;
  sockets: number;
  connects: number;
  logLength: number;
  reqs: { subId: string; live: boolean }[];
  quietForMs: number;
};

function readFixture(): Fixture {
  return JSON.parse(readFileSync(FIXTURE_URL, "utf8")) as Fixture;
}

/** Read-modify-write of the "desktop" key only; mobile writes its own key. */
function writeDesktop(mutate: (desktop: Fixture["desktop"]) => void) {
  const fixture = readFixture();
  mutate(fixture.desktop);
  writeFileSync(FIXTURE_URL, `${JSON.stringify(fixture, null, 2)}\n`);
}

function expectedBudget(): DesktopBudget {
  const desktop = readFixture().desktop as DesktopBudget;
  if (!desktop.checkpoints || !desktop.launch || !desktop.threadRows) {
    throw new Error(
      "live-req-budget.json has no measured desktop budget. Run with KUZZ_UPDATE_REQ_BUDGET=1.",
    );
  }
  return desktop;
}

async function snapshot(page: Page): Promise<Snapshot> {
  return page.evaluate((rowSelector) => {
    const open = window.__KUZZ_E2E_LIVE_REQS__?.() ?? [];
    const log = window.__KUZZ_E2E_REQ_LOG__?.() ?? [];
    const lastWire = [...log]
      .reverse()
      .find((entry) => entry.type !== "CONNECT" && entry.type !== "DISCONNECT");
    return {
      live: open
        .filter((sub) => sub.subIdClass === "live")
        .map(({ subId, signature, filters }) => ({
          subId,
          signature,
          filters,
        })),
      other: open
        .filter((sub) => sub.subIdClass !== "live")
        .map(({ subId, subIdClass, signature }) => ({
          subId,
          subIdClass,
          signature,
        })),
      // Same evaluate as the live list, so virtualization can't move between.
      mountedRows: document.querySelectorAll(rowSelector).length,
      sockets: window.__KUZZ_E2E_MOCK_SOCKET_COUNT__?.() ?? -1,
      connects:
        window.__KUZZ_E2E_GET_WEBSOCKET_CONNECT_ATTEMPTS__?.().length ?? -1,
      logLength: log.length,
      reqs: log.flatMap((entry) =>
        entry.type === "REQ"
          ? [{ subId: entry.subId, live: entry.subId.startsWith("live-") }]
          : [],
      ),
      quietForMs: lastWire ? Date.now() - lastWire.at : Date.now(),
    };
  }, SUMMARY_IN_TIMELINE);
}

function sortedSignatures(snap: Snapshot) {
  return snap.live.map((sub) => sub.signature).sort();
}

/** Multiset difference a − b. */
function minus(a: readonly string[], b: readonly string[]) {
  const rest = [...b];
  return a.filter((value) => {
    const index = rest.indexOf(value);
    if (index === -1) return true;
    rest.splice(index, 1);
    return false;
  });
}

function describeDiff(
  label: string,
  expected: readonly string[],
  measured: readonly string[],
) {
  const added = minus(measured, expected).map((sig) => `  + ${sig}`);
  const missing = minus(expected, measured).map((sig) => `  - ${sig}`);
  return [
    `[${label}] live subscriptions: expected ${expected.length}, measured ${measured.length}`,
    ...added,
    ...missing,
  ].join("\n");
}

/**
 * Fixture mode: poll until the live count equals `expected` and the wire has
 * been quiet for HOLD_MS. Update mode (`expected` null): quiet wire only.
 * `expected` may depend on the snapshot (mounted rows).
 */
async function settle(
  page: Page,
  label: string,
  testInfo: TestInfo,
  expected: number | null | ((snap: Snapshot) => number),
  expectedSignatures?: readonly string[],
): Promise<Snapshot> {
  const deadline = Date.now() + SETTLE_TIMEOUT_MS;
  let snap = await snapshot(page);
  let stableKey = "";
  let stableSince = Date.now();
  for (;;) {
    const target = typeof expected === "function" ? expected(snap) : expected;
    const key = `${snap.live.length}|${snap.mountedRows}`;
    if (key !== stableKey) {
      stableKey = key;
      stableSince = Date.now();
    }
    const countOk = target === null || snap.live.length === target;
    if (
      countOk &&
      snap.quietForMs >= HOLD_MS &&
      Date.now() - stableSince >= HOLD_MS
    ) {
      break;
    }
    if (Date.now() > deadline) {
      await testInfo.attach(`${label}-settle-timeout.json`, {
        body: JSON.stringify(snap, null, 2),
        contentType: "application/json",
      });
      const detail = expectedSignatures
        ? describeDiff(label, expectedSignatures, sortedSignatures(snap))
        : sortedSignatures(snap).join("\n");
      throw new Error(
        `[${label}] did not settle in ${SETTLE_TIMEOUT_MS} ms: live ${snap.live.length}, expected ${String(target)}, quiet ${snap.quietForMs} ms\n${detail}`,
      );
    }
    await page.waitForTimeout(250);
    snap = await snapshot(page);
  }
  await testInfo.attach(`${label}.json`, {
    body: JSON.stringify(
      { live: sortedSignatures(snap), other: snap.other, snap },
      null,
      2,
    ),
    contentType: "application/json",
  });
  // One socket, never reconnected, and every open id has a known class.
  expect(snap.sockets, `[${label}] open mock sockets`).toBe(1);
  expect(snap.connects, `[${label}] websocket connects`).toBe(1);
  const unclassified = snap.other.filter(
    (sub) => sub.subIdClass === "unclassified",
  );
  expect(
    unclassified,
    `[${label}] unclassified open subscription(s): ${unclassified.map((sub) => `${sub.subId} ${sub.signature}`).join(", ")}`,
  ).toEqual([]);
  return snap;
}

function assertCheckpoint(label: string, snap: Snapshot, expected: Checkpoint) {
  const measured = sortedSignatures(snap);
  expect(measured, describeDiff(label, expected.signatures, measured)).toEqual(
    expected.signatures,
  );
}

async function checkpoint(
  page: Page,
  name: CheckpointName,
  testInfo: TestInfo,
  measured: Partial<Record<CheckpointName, Checkpoint>>,
) {
  const expected = UPDATE ? null : expectedBudget().checkpoints[name];
  const snap = await settle(
    page,
    name,
    testInfo,
    expected?.live ?? null,
    expected?.signatures,
  );
  if (expected) assertCheckpoint(name, snap, expected);
  measured[name] = {
    live: snap.live.length,
    signatures: sortedSignatures(snap),
  };
  return snap;
}

// Desktop has no room-less chat view: "Home is Chats" (AppShell), so a launch
// lands on the first sidebar chat even from "#/". Idle is that landing, and
// its count includes the landing room's room-level subscriptions.
const LANDING_ROOM = "agents";

async function openHome(page: Page) {
  await installMockBridge(page);
  await page.goto("/");
  await expect(page.getByTestId("channel-general")).toBeVisible();
}

/** Pin the landing room, so a landing change shows up as a named failure. */
async function idleCheckpoint(
  page: Page,
  testInfo: TestInfo,
  measured: Partial<Record<CheckpointName, Checkpoint>>,
) {
  const title = page.getByTestId("chat-title");
  await expect(title).toHaveText(LANDING_ROOM);
  const snap = await checkpoint(page, "idle", testInfo, measured);
  await expect(title).toHaveText(LANDING_ROOM);
  await expect(page.getByTestId("message-thread-panel")).toHaveCount(0);
  return snap;
}

async function openRoom(page: Page, name: string) {
  // Seeding moves the room up the recency-sorted sidebar; a click during
  // that reorder can miss, so retry until the room is open.
  await expect(async () => {
    await page.getByTestId(`channel-${name}`).click({ timeout: 2_000 });
    await expect(page.getByTestId("chat-title")).toHaveText(name, {
      timeout: 2_000,
    });
  }).toPass({ timeout: 15_000 });
}

async function seedThreads(page: Page, channelName: string, count: number) {
  return page.evaluate(
    ({ channelName, count }) => {
      const emit = window.__KUZZ_E2E_EMIT_MOCK_MESSAGE__;
      if (!emit) throw new Error("mock emit unavailable");
      const heads: string[] = [];
      for (let index = 0; index < count; index += 1) {
        const head = emit({ channelName, content: `Budget head ${index}` });
        emit({
          channelName,
          content: `Budget reply ${index}`,
          parentEventId: head.id,
        });
        heads.push(head.id);
      }
      return heads;
    },
    { channelName, count },
  );
}

test.describe("live REQ budget", () => {
  test("idle, rooms, thread and DM hold their live subscription budget", async ({
    page,
  }, testInfo) => {
    test.setTimeout(120_000);
    const measured: Partial<Record<CheckpointName, Checkpoint>> = {};
    await openHome(page);
    const idle = await idleCheckpoint(page, testInfo, measured);
    expect(
      idle.live.some((sub) => sub.signature.includes("#h:")),
      "room bundle live REQ is open at idle",
    ).toBe(true);
    const launch = {
      oneShot: idle.reqs.filter((req) => !req.live).length,
      liveSends: idle.reqs.filter((req) => req.live).length,
    };
    await testInfo.attach("launch.json", {
      body: JSON.stringify(launch),
      contentType: "application/json",
    });
    if (!UPDATE) {
      const ceiling = expectedBudget().launch;
      expect(launch.oneShot, "launch one-shot REQs").toBeLessThanOrEqual(
        ceiling.oneShot,
      );
      expect(launch.liveSends, "launch live REQ sends").toBeLessThanOrEqual(
        ceiling.liveSends,
      );
    }

    // Seed B's thread before B is ever opened.
    const [threadHead] = await seedThreads(page, ROOM_B, 1);

    await openRoom(page, ROOM_A);
    await expect(page.locator(SUMMARY_IN_TIMELINE)).toHaveCount(0);
    await checkpoint(page, "roomA", testInfo, measured);

    await openRoom(page, ROOM_B);
    const row = page.locator(
      `${SUMMARY_IN_TIMELINE}[data-thread-head-id="${threadHead}"]`,
    );
    await expect(row).toBeVisible();
    await expect(page.locator(SUMMARY_IN_TIMELINE)).toHaveCount(1);
    await checkpoint(page, "roomB", testInfo, measured);

    await row.click();
    await expect(page.getByTestId("message-thread-panel")).toBeVisible();
    await checkpoint(page, "threadOpen", testInfo, measured);

    await page.getByTestId(`channel-${DM}`).click();
    await expect(page.getByTestId("chat-title")).toBeVisible();
    await checkpoint(page, "dmOpen", testInfo, measured);

    if (UPDATE) {
      writeDesktop((desktop) => {
        const previous = (desktop.launch ?? { oneShot: 0, liveSends: 0 }) as {
          oneShot: number;
          liveSends: number;
        };
        desktop.launch = {
          oneShot: Math.max(previous.oneShot, launch.oneShot),
          liveSends: Math.max(previous.liveSends, launch.liveSends),
        };
        desktop.checkpoints = measured as Record<CheckpointName, Checkpoint>;
      });
    }
  });

  test("each mounted thread row costs a fixed number of live subscriptions", async ({
    browser,
  }, testInfo) => {
    test.setTimeout(120_000);
    const budget = UPDATE ? null : expectedBudget();

    const measureRows = async (
      seeded: number,
      expected: number | null | ((snap: Snapshot) => number),
      expectedSignatures?: readonly string[],
    ) => {
      const context = await browser.newContext({
        viewport: ROWS_VIEWPORT,
      });
      try {
        const page = await context.newPage();
        await openHome(page);
        await idleCheckpoint(page, testInfo, {});
        await seedThreads(page, ROOM_C, seeded);
        await openRoom(page, ROOM_C);
        await expect
          .poll(() => page.locator(SUMMARY_IN_TIMELINE).count())
          .toBeGreaterThanOrEqual(10);
        return await settle(
          page,
          `rows${seeded}`,
          testInfo,
          expected,
          expectedSignatures,
        );
      } finally {
        await context.close();
      }
    };

    const rows10 = await measureRows(
      10,
      budget?.threadRows.rows10.live ?? null,
      budget?.threadRows.rows10.signatures,
    );
    expect(rows10.mountedRows, "all 10 seeded rows mount").toBe(10);
    if (budget) {
      assertCheckpoint("rows10", rows10, budget.threadRows.rows10);
    }

    const perRow = budget?.threadRows.livePerMountedRow;
    const rows50 = await measureRows(
      50,
      perRow === undefined
        ? null
        : (snap) => rows10.live.length + perRow * (snap.mountedRows - 10),
    );
    const extraRows = rows50.mountedRows - rows10.mountedRows;
    expect(extraRows, "50 seeded rows mount more than 10").toBeGreaterThan(0);
    const extraLive = rows50.live.length - rows10.live.length;
    const perRowSignatures = minus(
      sortedSignatures(rows50),
      sortedSignatures(rows10),
    );
    await testInfo.attach("rows-slope.json", {
      body: JSON.stringify({
        mounted10: rows10.mountedRows,
        mounted50: rows50.mountedRows,
        live10: rows10.live.length,
        live50: rows50.live.length,
        perRowSignatures: [...new Set(perRowSignatures)],
      }),
      contentType: "application/json",
    });
    expect(
      minus(sortedSignatures(rows10), sortedSignatures(rows50)),
      "rows10 live set is contained in rows50",
    ).toEqual([]);

    if (budget) {
      expect(
        extraLive,
        `live(50) − live(10) == ${budget.threadRows.livePerMountedRow} × (${rows50.mountedRows} − ${rows10.mountedRows})`,
      ).toBe(budget.threadRows.livePerMountedRow * extraRows);
      expect(
        [...new Set(perRowSignatures)].sort(),
        "per-row live subscription shapes",
      ).toEqual(budget.threadRows.perRowSignatures);
    } else {
      expect(extraLive % extraRows, "per-row cost is a whole number").toBe(0);
      writeDesktop((desktop) => {
        desktop.threadRows = {
          rows10: {
            live: rows10.live.length,
            mountedRows: rows10.mountedRows,
            signatures: sortedSignatures(rows10),
          },
          livePerMountedRow: extraLive / extraRows,
          perRowSignatures: [...new Set(perRowSignatures)].sort(),
        };
      });
    }
  });
});
