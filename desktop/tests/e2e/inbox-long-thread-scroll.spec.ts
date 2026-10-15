import { expect, test, type Page } from "@playwright/test";

import { TEST_IDENTITIES, installMockBridge } from "../helpers/bridge";

/**
 * Repro probe: a long channel thread (root + 120 markdown-heavy replies, the
 * latest few mentioning the current user) opened from the Inbox must scroll
 * back up to its root, both in the chat-room thread panel (mention rows) and
 * in the Inbox's own detail pane (needs-action rows), including while new
 * replies keep arriving.
 */

const GENERAL_CHANNEL_ID = "9a1657ac-f7aa-5db0-b632-d8bbeb6dfb50";
const INBOX_FILTER_STORAGE_KEY = "buzz.desktop.inbox-filter";
const REPLY_COUNT = 120;
const ROOT_ID = "1f".repeat(32);
const ROOT_TEXT = "LT thread root";

type RelayEventLike = {
  id: string;
  created_at: number;
  kind: number;
  pubkey: string;
  content: string;
  tags: string[][];
};

type MockWindow = Window & {
  __BUZZ_E2E_EMIT_MOCK_MESSAGE__?: (input: {
    channelName: string;
    content: string;
    parentEventId?: string;
    pubkey?: string;
    createdAt?: number;
    id?: string;
    mentionPubkeys?: string[];
  }) => RelayEventLike;
  __BUZZ_E2E_PUSH_MOCK_FEED_ITEM__?: (item: {
    category: "mention" | "needs_action" | "activity" | "agent_activity";
    channel_id: string | null;
    channel_name: string;
    content: string;
    created_at: number;
    id: string;
    kind: number;
    pubkey: string;
    tags: string[][];
  }) => unknown;
};

type Category = "mention" | "needs_action";

/** Seeds root + long markdown replies; returns the newest mention reply. */
async function seedLongMarkdownThread(page: Page) {
  await page.waitForFunction(
    () =>
      typeof (window as MockWindow).__BUZZ_E2E_EMIT_MOCK_MESSAGE__ ===
      "function",
  );
  return page.evaluate(
    ({ rootId, rootText, replies, alice, tyler }) => {
      const emit = (window as MockWindow).__BUZZ_E2E_EMIT_MOCK_MESSAGE__;
      if (!emit) throw new Error("emitter missing");
      const base = Math.floor(Date.now() / 1000) - 10_000;
      emit({
        channelName: "general",
        content: `${rootText}\n\nKickoff for the long review thread.`,
        createdAt: base,
        id: rootId,
        pubkey: alice,
      });
      const longBody = (i: number) => {
        const rows = Array.from(
          { length: 12 },
          (_, r) =>
            `| row ${r} | value ${i}-${r} | some longer cell text ${r} |`,
        ).join("\n");
        const code = Array.from(
          { length: 25 },
          (_, l) => `  const line${l} = compute(${i}, ${l}); // detail ${l}`,
        ).join("\n");
        const para =
          "Lorem ipsum dolor sit amet, consectetur adipiscing. ".repeat(12);
        return `## Section ${i}\n\n${para}\n\n| a | b | c |\n|---|---|---|\n${rows}\n\n\`\`\`ts\nfunction f${i}() {\n${code}\n}\n\`\`\`\n\n- point one\n- point two\n- point three\n\n${para}`;
      };
      let latest: RelayEventLike | null = null;
      for (let i = 0; i < replies; i += 1) {
        const marker = `LT reply ${String(i).padStart(3, "0")}`;
        const isLong = i % 4 === 0 || i >= replies - 6;
        const mention = i >= replies - 3;
        latest = emit({
          channelName: "general",
          content: isLong
            ? `${marker}\n\n${longBody(i)}`
            : `${marker}\n\nShort follow-up ${i}. ${"word ".repeat(40)}`,
          parentEventId: rootId,
          createdAt: base + 10 + i,
          pubkey: alice,
          mentionPubkeys: mention ? [tyler] : undefined,
        });
      }
      return latest;
    },
    {
      rootId: ROOT_ID,
      rootText: ROOT_TEXT,
      replies: REPLY_COUNT,
      alice: TEST_IDENTITIES.alice.pubkey,
      tyler: TEST_IDENTITIES.tyler.pubkey,
    },
  );
}

async function pushFeed(page: Page, event: RelayEventLike, category: Category) {
  await page.evaluate(
    ({ ev, channelId, cat }) => {
      (window as MockWindow).__BUZZ_E2E_PUSH_MOCK_FEED_ITEM__?.({
        category: cat,
        channel_id: channelId,
        channel_name: "general",
        content: ev.content,
        created_at: ev.created_at,
        id: ev.id,
        kind: ev.kind,
        pubkey: ev.pubkey,
        tags: ev.tags,
      });
    },
    { ev: event, channelId: GENERAL_CHANNEL_ID, cat: category },
  );
}

async function metrics(page: Page, testId: string) {
  return page.getByTestId(testId).evaluate((el, rootText) => {
    const scroller = el as HTMLElement;
    const rect = scroller.getBoundingClientRect();
    const markers = Array.from(scroller.querySelectorAll("p, div, span"))
      .filter(
        (node) =>
          node.childElementCount === 0 &&
          /^LT reply \d{3}$/.test(node.textContent?.trim() ?? ""),
      )
      .map((node) => node.textContent?.trim() ?? "");
    const rootNode = Array.from(scroller.querySelectorAll("*")).find(
      (node) =>
        node.childElementCount === 0 && node.textContent?.trim() === rootText,
    );
    const rootRect = rootNode?.getBoundingClientRect();
    return {
      scrollTop: Math.round(scroller.scrollTop),
      scrollHeight: scroller.scrollHeight,
      clientHeight: scroller.clientHeight,
      renderedReplies: markers.length,
      earliestRendered: markers[0] ?? null,
      rootInDom: Boolean(rootNode),
      rootVisible: Boolean(
        rootRect && rootRect.bottom > rect.top && rootRect.top < rect.bottom,
      ),
    };
  }, ROOT_TEXT);
}

/** Wheel up repeatedly; returns the scrollTop trajectory. */
async function wheelUp(page: Page, testId: string, ticks: number) {
  const body = page.getByTestId(testId);
  await body.hover();
  const trail: string[] = [];
  for (let i = 0; i < ticks; i += 1) {
    await page.mouse.wheel(0, -1200);
    await page.waitForTimeout(120);
    trail.push(
      await body.evaluate(
        (el) =>
          `${Math.round((el as HTMLElement).scrollTop)}/${(el as HTMLElement).scrollHeight}`,
      ),
    );
  }
  return trail;
}

/** Programmatic scrollTop write (no wheel), like a scrollbar drag. */
async function setScrollTopZero(page: Page, testId: string) {
  const body = page.getByTestId(testId);
  const samples: number[] = [];
  for (let i = 0; i < 15; i += 1) {
    await body.evaluate((el) => {
      (el as HTMLElement).scrollTop = 0;
    });
    await page.waitForTimeout(150);
    samples.push(
      await body.evaluate((el) => Math.round((el as HTMLElement).scrollTop)),
    );
  }
  return samples;
}

async function emitLiveReplies(page: Page, count: number, startIndex: number) {
  await page.evaluate(
    ({ rootId, alice, n, start }) => {
      const emit = (window as MockWindow).__BUZZ_E2E_EMIT_MOCK_MESSAGE__;
      for (let i = 0; i < n; i += 1) {
        emit?.({
          channelName: "general",
          content: `LT live ${start + i}\n\n## Live heading\n\n${"live text ".repeat(80)}`,
          parentEventId: rootId,
          pubkey: alice,
        });
      }
    },
    {
      rootId: ROOT_ID,
      alice: TEST_IDENTITIES.alice.pubkey,
      n: count,
      start: startIndex,
    },
  );
}

async function openFromInbox(
  page: Page,
  category: Category,
  filter: string,
  threadRepliesDelayMs = 0,
) {
  await page.addInitScript(
    ({ key, value }) => window.localStorage.setItem(key, value),
    { key: INBOX_FILTER_STORAGE_KEY, value: filter },
  );
  await installMockBridge(page, { threadRepliesDelayMs });
  await page.goto("/");
  const latest = await seedLongMarkdownThread(page);
  if (!latest) throw new Error("no reply seeded");
  await pushFeed(page, latest, category);
  const row = page.getByTestId(`home-inbox-item-${latest.id}`);
  await expect(row).toBeVisible({ timeout: 15_000 });
  await row.click();
}

const PATHS = [
  {
    name: "thread panel (mention row, All filter)",
    category: "mention" as const,
    filter: "all",
    scroller: "message-thread-body",
  },
  {
    name: "inbox detail pane (needs-action row)",
    category: "needs_action" as const,
    filter: "needs_action",
    scroller: "home-inbox-detail-scroll",
    // useInboxThreadContext loads only the newest THREAD_CONTEXT_LIMIT (100)
    // replies and has no paging, so the oldest replies never arrive here.
    fixme: true,
  },
];

for (const path of PATHS) {
  for (const live of [false, true]) {
    test(`long thread scrolls back to root: ${path.name}${live ? " + live replies" : ""}`, async ({
      page,
    }) => {
      test.fixme(path.fixme === true, "Inbox detail pane has no reply paging");
      test.setTimeout(120_000);
      await openFromInbox(page, path.category, path.filter);
      const scroller = page.getByTestId(path.scroller);
      await expect(scroller).toBeVisible({ timeout: 15_000 });
      await expect(scroller).toContainText(
        `LT reply ${String(REPLY_COUNT - 1).padStart(3, "0")}`,
      );
      await page.waitForTimeout(1_500);
      const opened = await metrics(page, path.scroller);
      console.log(`[${path.name}${live ? "+live" : ""}] opened`, opened);

      let liveTimer = 0;
      const pump = async () => {
        if (!live) return;
        await emitLiveReplies(page, 1, liveTimer);
        liveTimer += 1;
      };

      // Wheel phase.
      const trail: string[] = [];
      for (let round = 0; round < 8; round += 1) {
        await pump();
        trail.push(...(await wheelUp(page, path.scroller, 5)));
      }
      const afterWheel = await metrics(page, path.scroller);
      console.log(
        `[${path.name}${live ? "+live" : ""}] wheel trail`,
        JSON.stringify(trail),
      );
      console.log(
        `[${path.name}${live ? "+live" : ""}] after wheel`,
        afterWheel,
      );

      // Programmatic scrollTop phase (scrollbar-drag analogue).
      await pump();
      const zeroSamples = await setScrollTopZero(page, path.scroller);
      const afterZero = await metrics(page, path.scroller);
      console.log(
        `[${path.name}${live ? "+live" : ""}] scrollTop=0 samples`,
        JSON.stringify(zeroSamples),
      );
      console.log(
        `[${path.name}${live ? "+live" : ""}] after scrollTop=0`,
        afterZero,
      );

      const diag = await page.evaluate(() => ({
        olderSentinel: document.querySelectorAll(
          '[data-testid="message-thread-older-replies"]',
        ).length,
        threadWindowCalls: (
          (
            window as unknown as {
              __BUZZ_E2E_COMMAND_PAYLOADS__?: Array<{
                command: string;
                payload: unknown;
              }>;
            }
          ).__BUZZ_E2E_COMMAND_PAYLOADS__ ?? []
        )
          .filter((entry) => entry.command === "get_thread_windows")
          .map((entry) => JSON.stringify(entry.payload)),
      }));
      console.log(`[${path.name}${live ? "+live" : ""}] diag`, diag);

      expect(
        afterWheel.scrollTop < opened.scrollTop || afterZero.rootVisible,
      ).toBe(true);
      expect(afterZero.rootVisible).toBe(true);
      expect(afterZero.earliestRendered).toBe("LT reply 000");
    });
  }
}

test("thread panel: realistic wheel scroll-up pages every older reply", async ({
  page,
}) => {
  test.setTimeout(180_000);
  await openFromInbox(page, "mention", "all", 400);
  const scroller = "message-thread-body";
  await expect(page.getByTestId(scroller)).toContainText(
    `LT reply ${String(REPLY_COUNT - 1).padStart(3, "0")}`,
  );
  await page.waitForTimeout(1_000);
  const body = page.getByTestId(scroller);
  await body.hover();
  const trail: string[] = [];
  let topSince = 0;
  let lastHeight = 0;
  let prevTop = 0;
  for (let i = 0; i < 900 && topSince < 40; i += 1) {
    await page.mouse.wheel(0, -100);
    await page.waitForTimeout(16);
    const sample = await body.evaluate((el) => {
      const s = el as HTMLElement;
      return { top: Math.round(s.scrollTop), height: s.scrollHeight };
    });
    if (sample.height !== lastHeight || (sample.top === 0 && topSince === 0)) {
      trail.push(
        `#${i} ${prevTop}->${sample.top}/${lastHeight}->${sample.height}`,
      );
    }
    lastHeight = sample.height;
    prevTop = sample.top;
    topSince = sample.top === 0 ? topSince + 1 : 0;
  }
  const end = await metrics(page, scroller);
  console.log("[realistic] transitions", JSON.stringify(trail));
  console.log("[realistic] end", end, {
    olderSentinel: await page
      .getByTestId("message-thread-older-replies")
      .count(),
  });
  expect(end.earliestRendered).toBe("LT reply 000");
});
