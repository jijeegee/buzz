import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  eventMatchesFilter,
  ME_ADDRESSED_KINDS,
  ME_AUTHORED_KINDS,
  MeLiveBundle,
  meScopeOf,
} from "./relayMeBundle.ts";
import { RECONNECT_REPLAY_CHANNEL_LOOKBACK_SECS } from "./relayReconnectReplay.ts";

const fixtures = JSON.parse(
  readFileSync(
    new URL("../../../../test-fixtures/me-live-bundle.json", import.meta.url),
    "utf8",
  ),
);

const ME = "a".repeat(64);

const flush = async () => {
  for (let i = 0; i < 20; i++) await Promise.resolve();
};

/** A bundle over a fake relay; `opened` REQs resolve when released. */
function harness() {
  const clock = { now: 0 };
  const opened = [];
  const fetches = [];
  const timers = new Map();
  let nextTimer = 1;
  let fetchFailures = 0;
  let fetchResult = [];
  const bundle = new MeLiveBundle({
    open: (filter, onEvent, onReady, signal, onRemoved) =>
      new Promise((resolve, reject) => {
        const req = { filter, onEvent, closed: false, signal, onRemoved };
        req.release = () => {
          onReady("eose");
          resolve(async () => {
            req.closed = true;
          });
        };
        req.fail = () => reject(new Error("fixture REQ failure"));
        opened.push(req);
      }),
    fetch: async (filter) => {
      fetches.push(filter);
      if (fetchFailures > 0) {
        fetchFailures -= 1;
        throw new Error("fixture fetch failure");
      }
      return fetchResult;
    },
    setTimeout: (fn) => {
      const id = nextTimer++;
      timers.set(id, fn);
      return id;
    },
    nowSecs: () => clock.now,
  });
  return {
    clock,
    bundle,
    opened,
    fetches,
    runTimers() {
      const due = [...timers.values()];
      timers.clear();
      for (const fn of due) fn();
    },
    failFetches(count) {
      fetchFailures = count;
    },
    setFetchResult(events) {
      fetchResult = events;
    },
  };
}

function event(id, kind, tags, { pubkey = ME, createdAt = 100 } = {}) {
  return {
    id,
    pubkey,
    kind,
    created_at: createdAt,
    tags,
    content: "",
    sig: "",
  };
}

test("shared fixture: kinds and lookback match mobile", () => {
  assert.deepEqual([...ME_AUTHORED_KINDS], fixtures.authoredKinds);
  assert.deepEqual([...ME_ADDRESSED_KINDS], fixtures.addressedKinds);
  assert.equal(
    RECONNECT_REPLAY_CHANNEL_LOOKBACK_SECS,
    fixtures.catchUpLookbackSecs,
  );
});

for (const fixture of fixtures.scopes) {
  test(`shared fixture: ${fixture.name}`, () => {
    assert.deepEqual(meScopeOf(fixture.filter), fixture.scope);
  });
}

for (const fixture of fixtures.matching) {
  test(`shared fixture: ${fixture.name}`, () => {
    assert.equal(
      eventMatchesFilter(fixture.event, fixture.filter),
      fixture.matches,
    );
  });
}

test("listeners of one scope share one REQ and get only their documents", async () => {
  const { bundle, opened } = harness();
  const mutes = [];
  const stars = [];
  const muting = bundle.subscribe(
    { kinds: [30078], authors: [ME], "#d": ["channel-mutes"], limit: 0 },
    (e) => mutes.push(e.id),
  );
  const starring = bundle.subscribe(
    { kinds: [30078], authors: [ME], "#d": ["channel-stars"], limit: 0 },
    (e) => stars.push(e.id),
  );
  await flush();
  assert.equal(opened.length, 1);
  assert.deepEqual(opened[0].filter, {
    kinds: fixtures.authoredKinds,
    authors: [ME],
    limit: 0,
  });
  opened[0].release();
  await Promise.all([muting, starring]);

  opened[0].onEvent(event("m1", 30078, [["d", "channel-mutes"]]));
  opened[0].onEvent(event("s1", 30078, [["d", "channel-stars"]]));
  opened[0].onEvent(event("m1", 30078, [["d", "channel-mutes"]]));
  assert.deepEqual(mutes, ["m1"]);
  assert.deepEqual(stars, ["s1"]);
});

test("a listener is ready once the scope's REQ is, with no history", async () => {
  const { bundle, opened, fetches } = harness();
  let ready = null;
  const subscribing = bundle.subscribe(
    { kinds: [44100, 44101], "#p": [ME], limit: 0 },
    () => {},
    (r) => {
      ready = r;
    },
  );
  await flush();
  assert.deepEqual(opened[0].filter, {
    kinds: fixtures.addressedKinds,
    "#p": [ME],
    limit: 0,
  });
  assert.equal(ready, null);
  opened[0].release();
  await subscribing;
  assert.equal(ready, "eose");
  assert.deepEqual(fetches, []);
});

test("the last listener leaving closes the REQ unless one comes straight back", async () => {
  const { bundle, opened } = harness();
  const filter = { kinds: [24210], "#p": [ME], limit: 0 };
  const subscribing = bundle.subscribe(filter, () => {});
  await flush();
  opened[0].release();
  const stop = await subscribing;

  // An effect re-run: cleanup, then subscribe again in the same task.
  void stop();
  const again = await bundle.subscribe(filter, () => {});
  await flush();
  assert.equal(opened.length, 1);
  assert.equal(opened[0].closed, false);

  await again();
  await flush();
  assert.equal(opened[0].closed, true);
  bundle.subscribe(filter, () => {});
  await flush();
  assert.equal(opened.length, 2, "a later listener opens a fresh REQ");
});

test("a failed REQ rejects its listeners and the next one retries", async () => {
  const { bundle, opened } = harness();
  const filter = { kinds: [24211], "#p": [ME], limit: 0 };
  const first = bundle.subscribe(filter, () => {});
  await flush();
  opened[0].fail();
  await assert.rejects(first, /fixture REQ failure/);

  const second = bundle.subscribe(filter, () => {});
  await flush();
  assert.equal(opened.length, 2);
  opened[1].release();
  await second;
});

test("reconnect catch-up delivers what each listener missed, once", async () => {
  const { bundle, opened, fetches, setFetchResult, failFetches, runTimers } =
    harness();
  const names = [];
  const subscribing = bundle.subscribe(
    { kinds: [30078], authors: [ME], "#d": ["channel-names"], limit: 0 },
    (e) => names.push(e.id),
  );
  await flush();
  opened[0].release();
  await subscribing;
  opened[0].onEvent(event("n1", 30078, [["d", "channel-names"]]));

  // Every attempt fails: the window is owed to the next reconnect.
  failFetches(3);
  const failing = bundle.catchUp(1_000);
  for (let i = 0; i < 5; i++) {
    await flush();
    runTimers();
  }
  await failing;
  assert.equal(fetches.length, 3);

  setFetchResult([
    event("n1", 30078, [["d", "channel-names"]]),
    event("n2", 30078, [["d", "channel-names"]], { createdAt: 1_500 }),
    event("x", 30078, [["d", "channel-sort"]]),
  ]);
  await bundle.catchUp(2_000);
  assert.deepEqual(fetches.at(-1), {
    kinds: fixtures.authoredKinds,
    authors: [ME],
    limit: fixtures.catchUpLimit,
    since: 1_000,
  });
  assert.deepEqual(names, ["n1", "n2"]);

  await bundle.catchUp(3_000);
  assert.equal(fetches.at(-1).since, 3_000, "the owed window was paid");
});

test("catch-up hands a listener nothing from before it opened", async () => {
  const { bundle, opened, fetches, setFetchResult, clock } = harness();
  const early = [];
  const late = [];
  clock.now = 1_000;
  const first = bundle.subscribe(
    { kinds: [24211], "#p": [ME], limit: 0 },
    (e) => early.push(e.id),
  );
  await flush();
  opened[0].release();
  await first;
  clock.now = 2_000;
  await bundle.subscribe({ kinds: [24211], "#p": [ME], limit: 0 }, (e) =>
    late.push(e.id),
  );

  setFetchResult([
    event("old", 24211, [["p", ME]], { createdAt: 1_500 }),
    event("skewed", 24211, [["p", ME]], { createdAt: 1_997 }),
    event("new", 24211, [["p", ME]], { createdAt: 2_100 }),
  ]);
  await bundle.catchUp(100);
  // The fetch starts at the earliest listener, less the skew allowance.
  assert.equal(fetches.at(-1).since, 995);
  assert.deepEqual(early, ["old", "skewed", "new"]);
  assert.deepEqual(late, ["skewed", "new"]);
});

test("a REQ the relay drops for good lets the next listener open a fresh one", async () => {
  const { bundle, opened } = harness();
  const filter = { kinds: [30078], authors: [ME], "#d": ["x"], limit: 0 };
  const subscribing = bundle.subscribe(filter, () => {});
  await flush();
  opened[0].release();
  await subscribing;

  opened[0].onRemoved();
  assert.equal(opened[0].signal.aborted, true);
  const again = bundle.subscribe(filter, () => {});
  await flush();
  assert.equal(opened.length, 2, "a fresh REQ");
  opened[1].release();
  await again;
});

test("reset drops every scope", async () => {
  const { bundle, opened } = harness();
  const seen = [];
  const subscribing = bundle.subscribe(
    { kinds: [3], authors: [ME], limit: 0 },
    (e) => seen.push(e.id),
  );
  await flush();
  opened[0].release();
  await subscribing;
  bundle.reset();
  assert.equal(opened[0].closed, true);
  opened[0].onEvent(event("c1", 3, []));
  assert.deepEqual(seen, []);
});
