import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { beforeEach, test } from "node:test";

import {
  COMMUNITY_ROW_FILTERS,
  CommunityLiveBundle,
  communityScopeOf,
  DESKTOP_COMMUNITY_ROWS,
  PRESENCE_AUTHOR_CAP,
} from "./relayCommunityBundle.ts";

const fixtures = JSON.parse(
  readFileSync(
    new URL(
      "../../../../test-fixtures/community-live-bundle.json",
      import.meta.url,
    ),
  ),
);

test("shared fixture: rows match", () => {
  assert.deepEqual(
    fixtures.rows.filter((row) => row.desktop).map((row) => row.name),
    [...DESKTOP_COMMUNITY_ROWS],
  );
  for (const row of fixtures.rows) {
    assert.deepEqual(COMMUNITY_ROW_FILTERS[row.name], row.filter, row.name);
  }
});

for (const fixture of fixtures.scopeCases) {
  test(`shared fixture: ${JSON.stringify(fixture.filter)}`, () => {
    assert.equal(communityScopeOf(fixture.filter), fixture.desktop);
  });
}

// --- A fake clock and a fake session REQ. ---
let now = 0;
let timers = new Map();
let nextTimer = 1;
let reqs = [];
let warnings = [];

function tick(ms) {
  const target = now + ms;
  for (;;) {
    const next = [...timers]
      .filter(([, timer]) => timer.at <= target)
      .sort((a, b) => a[1].at - b[1].at || a[0] - b[0])[0];
    if (!next) break;
    now = next[1].at;
    timers.delete(next[0]);
    next[1].fn();
  }
  now = target;
}
async function settle() {
  for (let i = 0; i < 10; i++) await Promise.resolve();
}
async function advance(ms) {
  // Step in small slices so promise callbacks run between timers.
  const end = now + ms;
  while (now < end) {
    tick(Math.min(50, end - now));
    await settle();
  }
}

function makeBundle(rows = DESKTOP_COMMUNITY_ROWS) {
  return new CommunityLiveBundle({
    rows,
    open: (filters, onEvent, onStatus, signal, onGone) => {
      const req = {
        filters,
        updates: [],
        onEvent,
        onStatus,
        onGone,
        signal,
        closed: false,
      };
      reqs.push(req);
      return new Promise((resolve, reject) => {
        req.fail = () => reject(new Error("open failed"));
        req.ready = () => {
          onStatus("ready");
          resolve({
            close: async () => {
              req.closed = true;
            },
            setFilters: async (next) => {
              req.updates.push({ at: now, filters: next });
            },
          });
        };
      });
    },
    setTimeout: (fn, ms) => {
      const id = nextTimer++;
      timers.set(id, { fn, at: now + ms });
      return id;
    },
    clearTimeout: (id) => timers.delete(id),
    nowMs: () => now,
    warn: (message) => warnings.push(message),
  });
}

const key = (i) => i.toString(16).padStart(64, "0");
const presenceOf = (filters) =>
  filters.find((filter) => filter.kinds[0] === 20001)?.authors ?? null;
const ev = (id, kind, pubkey, tags = []) => ({
  id,
  kind,
  pubkey,
  tags,
  content: "",
  created_at: 1,
  sig: "",
});

beforeEach(() => {
  now = 0;
  timers = new Map();
  nextTimer = 1;
  reqs = [];
  warnings = [];
});

async function openWith(bundle, listeners) {
  const stops = listeners.map(([filter, onEvent, options]) =>
    bundle.subscribe(filter, onEvent ?? (() => {}), options),
  );
  await advance(250);
  reqs.at(-1).ready();
  await settle();
  return Promise.all(stops);
}

test("one REQ for every listener, opened 250 ms after the first", async () => {
  const bundle = makeBundle();
  bundle.subscribe(COMMUNITY_ROW_FILTERS.status, () => {});
  await advance(100);
  bundle.subscribe(COMMUNITY_ROW_FILTERS.emoji, () => {});
  bundle.addPresenceAuthors([key(1).toUpperCase()]);
  await advance(149);
  assert.equal(reqs.length, 0);
  await advance(1);
  assert.equal(reqs.length, 1);
  assert.deepEqual(reqs[0].filters, [
    COMMUNITY_ROW_FILTERS.emoji,
    COMMUNITY_ROW_FILTERS.status,
    COMMUNITY_ROW_FILTERS.catalogs,
    { kinds: [20001], authors: [key(1)], limit: 0 },
  ]);
});

test("the presence row is omitted while the author set is empty", async () => {
  const bundle = makeBundle(["status", "presence"]);
  await openWith(bundle, [[COMMUNITY_ROW_FILTERS.status]]);
  assert.deepEqual(reqs[0].filters, [COMMUNITY_ROW_FILTERS.status]);
});

test("each listener hears only what its filter matches, once", async () => {
  const bundle = makeBundle();
  const persona = [];
  const team = [];
  const status = [];
  await openWith(bundle, [
    [{ kinds: [30175], limit: 0 }, (e) => persona.push(e.id)],
    [{ kinds: [30178], limit: 0 }, (e) => team.push(e.id)],
    [COMMUNITY_ROW_FILTERS.status, (e) => status.push(e.id)],
  ]);
  const req = reqs[0];
  req.onEvent(ev("p1", 30175, key(1)));
  req.onEvent(ev("t1", 30178, key(1)));
  req.onEvent(ev("s1", 30315, key(1), [["d", "general"]]));
  req.onEvent(ev("s1", 30315, key(1), [["d", "general"]]));
  req.onEvent(ev("s2", 30315, key(1), [["d", "music"]]));
  assert.deepEqual(persona, ["p1"]);
  assert.deepEqual(team, ["t1"]);
  assert.deepEqual(status, ["s1"]);
});

test("filters that are not community rows are refused", async () => {
  const bundle = makeBundle();
  await assert.rejects(
    bundle.subscribe({ kinds: [20001], authors: [key(1)], limit: 0 }, () => {}),
  );
});

test("the author set grows only, sorted, and never shrinks on stop", async () => {
  const bundle = makeBundle();
  const [stop] = await openWith(bundle, [[COMMUNITY_ROW_FILTERS.presence]]);
  bundle.addPresenceAuthors([key(3), key(1)]);
  await advance(250);
  assert.deepEqual(presenceOf(reqs[0].updates[0].filters), [key(1), key(3)]);
  bundle.addPresenceAuthors([key(1)]);
  await advance(60_000);
  assert.equal(reqs[0].updates.length, 1, "a known key sends nothing");
  await stop();
  await settle();
  assert.equal(reqs[0].closed, true, "closes one task after the last stop");
  bundle.subscribe(COMMUNITY_ROW_FILTERS.status, () => {});
  await advance(250);
  assert.deepEqual(presenceOf(reqs[1].filters), [key(1), key(3)]);
});

test("a stop and a subscribe in the same task keep the REQ", async () => {
  const bundle = makeBundle();
  const [stop] = await openWith(bundle, [[COMMUNITY_ROW_FILTERS.status]]);
  void stop(); // an effect cleanup, then the next effect, synchronously
  bundle.subscribe(COMMUNITY_ROW_FILTERS.status, () => {});
  await settle();
  await advance(1_000);
  assert.equal(reqs.length, 1);
  assert.equal(reqs[0].closed, false);
});

test("200 adds over 90 s: at most one re-REQ per 30 s; a quiet add goes at 250 ms", async () => {
  const bundle = makeBundle();
  await openWith(bundle, [[COMMUNITY_ROW_FILTERS.presence]]);
  const start = now;
  for (let i = 0; i < 200; i += 1) {
    bundle.addPresenceAuthors([key(100 + i)]);
    await advance(450);
  }
  await advance(30_000);
  const updates = reqs[0].updates;
  assert.equal(updates[0].at - start, 250, "the first add goes out at 250 ms");
  for (let i = 1; i < updates.length; i += 1) {
    assert.ok(
      updates[i].at - updates[i - 1].at >= 30_000,
      `re-REQ ${i} came ${updates[i].at - updates[i - 1].at} ms after the last`,
    );
  }
  assert.ok(updates.length <= 5, `${updates.length} re-REQs`);
  assert.equal(presenceOf(updates.at(-1).filters).length, 200);
  // A quiet period later, the next add goes out after 250 ms again.
  await advance(60_000);
  const before = updates.length;
  bundle.addPresenceAuthors([key(9_999)]);
  await advance(249);
  assert.equal(updates.length, before);
  await advance(1);
  assert.equal(updates.length, before + 1);
});

test("adds before the opening EOSE ride the opening REQ or follow it at 250 ms", async () => {
  const bundle = makeBundle();
  bundle.subscribe(COMMUNITY_ROW_FILTERS.presence, () => {});
  bundle.addPresenceAuthors([key(1)]);
  await advance(250);
  assert.deepEqual(presenceOf(reqs[0].filters), [key(1)]);
  bundle.addPresenceAuthors([key(2)]);
  await advance(5_000);
  assert.equal(reqs[0].updates.length, 0, "waits for the opening EOSE");
  reqs[0].ready();
  await settle();
  await advance(250);
  assert.equal(reqs[0].updates.length, 1);
  assert.deepEqual(presenceOf(reqs[0].updates[0].filters), [key(1), key(2)]);
});

test("the set is capped at 1000; the rest is refused once with a warning", async () => {
  const bundle = makeBundle();
  await openWith(bundle, [[COMMUNITY_ROW_FILTERS.presence]]);
  const keys = Array.from({ length: PRESENCE_AUTHOR_CAP + 5 }, (_, i) =>
    key(i + 1),
  );
  const refused = bundle.addPresenceAuthors(keys);
  assert.equal(refused.length, 5);
  assert.deepEqual(bundle.addPresenceAuthors([key(5_000)]), [key(5_000)]);
  assert.equal(warnings.length, 1);
  await advance(250);
  assert.equal(
    presenceOf(reqs[0].updates[0].filters).length,
    PRESENCE_AUTHOR_CAP,
  );
});

test("reset clears the set and asks listeners to add their keys again", async () => {
  const bundle = makeBundle();
  let resets = 0;
  await openWith(bundle, [
    [COMMUNITY_ROW_FILTERS.presence, () => {}, { onReset: () => resets++ }],
  ]);
  bundle.addPresenceAuthors([key(1)]);
  bundle.reset();
  assert.equal(resets, 1);
  assert.equal(reqs[0].closed, true);
  bundle.addPresenceAuthors([key(2)]);
  await advance(250);
  assert.equal(reqs.length, 2);
  assert.deepEqual(presenceOf(reqs[1].filters), [key(2)]);
});

test("a dropped REQ reopens with backoff; listeners hear retrying, then ready", async () => {
  const bundle = makeBundle();
  const statuses = [];
  await openWith(bundle, [
    [
      COMMUNITY_ROW_FILTERS.status,
      () => {},
      { onStatus: (s) => statuses.push(s) },
    ],
  ]);
  reqs[0].onGone();
  assert.deepEqual(statuses, ["ready", "retrying"]);
  await advance(999);
  assert.equal(reqs.length, 1);
  await advance(1);
  assert.equal(reqs.length, 2);
  reqs[1].fail();
  await settle();
  await advance(1_999);
  assert.equal(reqs.length, 2);
  await advance(1);
  assert.equal(reqs.length, 3, "1 s, then 2 s");
  reqs[2].ready();
  await settle();
  assert.deepEqual(statuses, ["ready", "retrying", "retrying", "ready"]);
  reqs[2].onGone();
  await advance(1_000);
  assert.equal(reqs.length, 4, "backoff starts over after a success");
});
