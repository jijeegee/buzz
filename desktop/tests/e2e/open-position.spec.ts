import { expect, type Page, test } from "@playwright/test";

import { installMockBridge, TEST_IDENTITIES } from "../helpers/bridge";

const GENERAL_CHANNEL_ID = "9a1657ac-f7aa-5db0-b632-d8bbeb6dfb50";
const STORE_PREFIX = "kuzz-e2e-message-store:";

type TestWindow = Window & {
  __KUZZ_E2E_EMIT_MOCK_MESSAGE__?: (input: {
    channelName: string;
    content: string;
    pubkey: string;
    createdAt?: number;
  }) => unknown;
  __KUZZ_E2E_EMIT_MOCK_READ_STATE__?: (input: {
    clientId: string;
    contexts: Record<string, number>;
    createdAt: number;
    slotId: string;
  }) => unknown;
  __KUZZ_E2E_HAS_MOCK_LIVE_SUBSCRIPTION__?: (input: {
    channelName: string;
    kind?: number;
  }) => boolean;
  __KUZZ_E2E_COMMAND_PAYLOADS__?: Array<{ command: string; payload: unknown }>;
  __KUZZ_E2E_QUERY_CLIENT__?: {
    removeQueries: (filters: { queryKey: readonly unknown[] }) => void;
  };
  __openFrames?: OpenFrame[];
  __openStop?: boolean;
};

/** One animation frame of the main timeline, null fields while hidden. */
type OpenFrame = {
  title: string | null;
  /** Last row's bottom minus the composer's top. */
  lastRowGap: number | null;
  /** The "New" divider's top within the timeline. */
  dividerTop: number | null;
};

async function waitForLive(page: Page, channelName: string, kind?: number) {
  await expect
    .poll(() =>
      page.evaluate(
        ({ name, kind }) =>
          (window as TestWindow).__KUZZ_E2E_HAS_MOCK_LIVE_SUBSCRIPTION__?.({
            channelName: name,
            kind,
          }) ?? false,
        { name: channelName, kind },
      ),
    )
    .toBe(true);
}

/** Emit `count` unread rows to general, a minute past the read frontier. */
async function emitUnread(page: Page, count: number) {
  await page.evaluate(
    ({ total, base, pubkey }) => {
      const emit = (window as TestWindow).__KUZZ_E2E_EMIT_MOCK_MESSAGE__;
      for (let index = 0; index < total; index += 1) {
        emit?.({
          channelName: "general",
          content: `Unread message ${index + 1}`,
          pubkey,
          createdAt: base + index,
        });
      }
    },
    {
      total: count,
      base: Math.floor(Date.now() / 1000) + 60,
      pubkey: TEST_IDENTITIES.alice.pubkey,
    },
  );
}

async function startSampler(page: Page) {
  await page.evaluate(() => {
    const sampler = window as TestWindow;
    sampler.__openFrames = [];
    sampler.__openStop = false;
    const step = () => {
      if (sampler.__openStop) return;
      const timeline = document.querySelector<HTMLElement>(
        '[data-testid="message-timeline"]',
      );
      // The landing hides the list host; visibility inherits down to rows.
      const rows = [
        ...(timeline?.querySelectorAll<HTMLElement>("[data-message-id]") ?? []),
      ].filter((row) => getComputedStyle(row).visibility !== "hidden");
      const visible = rows.length > 0;
      const last = rows.at(-1);
      const composer = document.querySelector<HTMLElement>(
        '[data-testid="message-composer"]',
      );
      const divider = visible
        ? document.querySelector<HTMLElement>(
            '[data-testid="message-unread-divider"]',
          )
        : null;
      sampler.__openFrames?.push({
        title:
          document.querySelector('[data-testid="chat-title"]')?.textContent ??
          null,
        lastRowGap:
          last && composer
            ? Math.round(
                last.getBoundingClientRect().bottom -
                  composer.getBoundingClientRect().top,
              )
            : null,
        dividerTop:
          divider && timeline
            ? Math.round(
                divider.getBoundingClientRect().top -
                  timeline.getBoundingClientRect().top,
              )
            : null,
      });
      requestAnimationFrame(step);
    };
    requestAnimationFrame(step);
  });
}

async function stopSampler(page: Page) {
  return page.evaluate(() => {
    const sampler = window as TestWindow;
    sampler.__openStop = true;
    return sampler.__openFrames ?? [];
  });
}

async function clearCommandLog(page: Page) {
  await page.evaluate(() => {
    const log = (window as TestWindow).__KUZZ_E2E_COMMAND_PAYLOADS__;
    if (log) log.length = 0;
  });
}

/** Older-page server requests (a cursor) since the log was cleared. */
async function olderServerPages(page: Page) {
  return page.evaluate(
    () =>
      ((window as TestWindow).__KUZZ_E2E_COMMAND_PAYLOADS__ ?? []).filter(
        (entry) =>
          entry.command === "get_channel_window" &&
          (entry.payload as { cursor?: unknown } | null)?.cursor != null,
      ).length,
  );
}

test("a room opening at its newest row shows its final position on the first visible frame", async ({
  page,
}) => {
  await installMockBridge(page);
  await page.goto("/");
  await page.getByTestId("channel-general").click();
  await expect(page.getByTestId("chat-title")).toHaveText("general");

  await startSampler(page);
  await page.getByTestId("channel-deep-history").click();
  await expect(page.getByTestId("chat-title")).toHaveText("deep-history");
  await expect(
    page.getByTestId("message-timeline").locator("[data-message-id]").first(),
  ).toBeVisible();
  await page.waitForTimeout(1_500);
  const frames = await stopSampler(page);

  const shown = frames.filter(
    (frame) => frame.title === "deep-history" && frame.lastRowGap !== null,
  );
  expect(shown.length).toBeGreaterThan(10);
  // The newest row sits above the composer from the first visible frame on.
  const final = shown.at(-1)?.lastRowGap ?? 0;
  expect(final).toBeLessThanOrEqual(1);
  expect(shown.map((frame) => frame.lastRowGap)).toEqual(
    shown.map(() => final),
  );
});

test("a long unread run opens at its first unread from stored pages, with no older server page", async ({
  page,
}) => {
  await installMockBridge(page);
  await page.goto("/");
  await page.getByTestId("channel-general").click();
  await expect(page.getByTestId("chat-title")).toHaveText("general");
  await waitForLive(page, "general");
  await page.getByTestId("channel-random").click();
  await expect(page.getByTestId("chat-title")).toHaveText("random");

  // Past the head and four older pages (5 x 50 rows): the open reads older
  // pages to reach the first unread. The store took every row live, so none
  // of them asks the server.
  await emitUnread(page, 300);
  await expect
    .poll(() =>
      page.evaluate(
        (prefix) =>
          Object.entries(window.localStorage)
            .filter(([key]) => key.startsWith(prefix))
            .some(([, value]) => value.includes("Unread message 300")),
        STORE_PREFIX,
      ),
    )
    .toBe(true);
  // Drop the room's in-memory rows, as after a restart: only the store has
  // them now.
  await page.evaluate((channelId) => {
    const client = (window as TestWindow).__KUZZ_E2E_QUERY_CLIENT__;
    client?.removeQueries({ queryKey: ["channel-window", channelId] });
    client?.removeQueries({ queryKey: ["channel-messages", channelId] });
  }, GENERAL_CHANNEL_ID);
  await clearCommandLog(page);
  await startSampler(page);
  await page.getByTestId("channel-general").click();
  await expect(page.getByTestId("message-unread-divider")).toBeVisible();
  await expect(
    page.getByText("Unread message 1", { exact: true }),
  ).toBeVisible();
  // The refresh after the live subscription settles replaces the head; the
  // rows below it must stay.
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          ((window as TestWindow).__KUZZ_E2E_COMMAND_PAYLOADS__ ?? []).filter(
            (entry) =>
              entry.command === "get_channel_window" &&
              (entry.payload as { cursor?: unknown } | null)?.cursor == null,
          ).length,
      ),
    )
    .toBeGreaterThan(0);
  await page.waitForTimeout(500);
  const frames = await stopSampler(page);

  await expect(
    page.getByText("Unread message 1", { exact: true }),
  ).toBeVisible();
  expect(await olderServerPages(page)).toBe(0);
  const general = frames.filter(
    (frame) => frame.title === "general" && frame.lastRowGap !== null,
  );
  expect(general.length).toBeGreaterThan(5);
  // Never painted at the bottom first: the divider is there from the first
  // visible frame and does not move.
  const first = general[0].dividerTop;
  expect(first).not.toBeNull();
  expect(general.map((frame) => frame.dividerTop)).toEqual(
    general.map(() => first),
  );
});

test("a newer read marker from another device keeps the open room's position and divider", async ({
  page,
}) => {
  await installMockBridge(page);
  await page.goto("/");
  await page.getByTestId("channel-general").click();
  await expect(page.getByTestId("chat-title")).toHaveText("general");
  await waitForLive(page, "general");
  await waitForLive(page, "general", 30078);
  await page.getByTestId("channel-random").click();
  await expect(page.getByTestId("chat-title")).toHaveText("random");

  await emitUnread(page, 40);
  await page.getByTestId("channel-general").click();
  const divider = page.getByTestId("message-unread-divider");
  await expect(divider).toBeVisible();
  await page.waitForTimeout(500);
  const timeline = page.getByTestId("message-timeline");
  const before = await timeline.evaluate((element) => element.scrollTop);

  await startSampler(page);
  // Another device read everything, well past the unread run.
  const readAt = Math.floor(Date.now() / 1000) + 600;
  await page.evaluate(
    ({ channelId, ts }) =>
      (window as TestWindow).__KUZZ_E2E_EMIT_MOCK_READ_STATE__?.({
        clientId: "other-device-client-id",
        slotId: "e2e00000000000000000000000000001",
        contexts: { [channelId]: ts },
        createdAt: ts,
      }),
    { channelId: GENERAL_CHANNEL_ID, ts: readAt },
  );
  await page.waitForTimeout(1_000);
  const frames = await stopSampler(page);

  await expect(divider).toBeVisible();
  expect(await timeline.evaluate((element) => element.scrollTop)).toBe(before);
  const tops = frames.map((frame) => frame.dividerTop);
  expect(new Set(tops)).toEqual(new Set([tops[0]]));
  expect(tops[0]).not.toBeNull();
});
