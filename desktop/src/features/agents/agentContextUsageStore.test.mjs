import assert from "node:assert/strict";
import { beforeEach, describe, it } from "node:test";

class MemoryStorage {
  values = new Map();
  getItem(key) {
    return this.values.get(key) ?? null;
  }
  setItem(key, value) {
    this.values.set(key, value);
  }
  removeItem(key) {
    this.values.delete(key);
  }
}

const storage = new MemoryStorage();
globalThis.window = { localStorage: storage };

const {
  MAX_CONTEXT_USAGE_ENTRIES,
  _testResetAgentContextUsageStore,
  contextUsageObserverListener,
  getAgentContextUsage,
  getAgentContextUsages,
  groupContextUsagesByChannel,
  ingestContextUsageEvents,
  parseContextUsageEvent,
  resolveMessageContextUsage,
  setContextUsageOwnerScope,
  subscribeAgentContextUsage,
} = await import("./agentContextUsageStore.ts");

const AGENT =
  "ABCD1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234";
const AGENT_KEY = AGENT.toLowerCase();
const AGENT_2 =
  "dcba4321dcba4321dcba4321dcba4321dcba4321dcba4321dcba4321dcba4321";
const OWNER =
  "1111111111111111111111111111111111111111111111111111111111111111";
const OWNER_2 =
  "2222222222222222222222222222222222222222222222222222222222222222";
const ROOT = "a".repeat(64);
const ROOT_2 = "b".repeat(64);

function usageEvent({
  seq = 1,
  timestamp = "2026-10-07T00:00:00.000Z",
  channelId = "chan-1",
  threadRootEventId = ROOT,
  used = 50_000,
  size = 200_000,
  compactSupported = true,
  sessionId = "sess-1",
  kind = "context_usage",
} = {}) {
  return {
    seq,
    timestamp,
    kind,
    agentIndex: 0,
    channelId,
    sessionId,
    turnId: null,
    payload: { sessionId, used, size, threadRootEventId, compactSupported },
  };
}

describe("agentContextUsageStore", () => {
  beforeEach(() => {
    storage.values.clear();
    _testResetAgentContextUsageStore();
  });

  describe("parseContextUsageEvent", () => {
    it("parses a well-formed reading and normalizes keys", () => {
      const reading = parseContextUsageEvent(
        AGENT,
        usageEvent({ threadRootEventId: ROOT.toUpperCase() }),
      );
      assert.deepEqual(reading, {
        agentPubkey: AGENT_KEY,
        channelId: "chan-1",
        threadRootEventId: ROOT,
        sessionId: "sess-1",
        used: 50_000,
        size: 200_000,
        compactSupported: true,
        updatedAt: Date.parse("2026-10-07T00:00:00.000Z"),
        timestamp: "2026-10-07T00:00:00.000Z",
        seq: 1,
      });
    });

    it("rejects other kinds, missing channel, and unusable windows", () => {
      assert.equal(
        parseContextUsageEvent(AGENT, usageEvent({ kind: "turn_started" })),
        null,
      );
      assert.equal(
        parseContextUsageEvent(AGENT, usageEvent({ channelId: null })),
        null,
      );
      assert.equal(
        parseContextUsageEvent(AGENT, usageEvent({ size: 0 })),
        null,
      );
      assert.equal(
        parseContextUsageEvent(AGENT, usageEvent({ used: -1 })),
        null,
      );
      assert.equal(
        parseContextUsageEvent(AGENT, usageEvent({ used: "5" })),
        null,
      );
    });

    it("treats a missing compactSupported as unsupported", () => {
      const event = usageEvent();
      delete event.payload.compactSupported;
      assert.equal(
        parseContextUsageEvent(AGENT, event)?.compactSupported,
        false,
      );
    });
  });

  describe("keying", () => {
    it("keys by agent, channel, and thread root (null distinct)", () => {
      ingestContextUsageEvents(AGENT, [
        usageEvent({ seq: 1, threadRootEventId: ROOT, used: 1 }),
        usageEvent({ seq: 2, threadRootEventId: ROOT_2, used: 2 }),
        usageEvent({ seq: 3, threadRootEventId: null, used: 3 }),
        usageEvent({ seq: 4, channelId: "chan-2", used: 4 }),
      ]);
      ingestContextUsageEvents(AGENT_2, [usageEvent({ seq: 1, used: 5 })]);

      assert.equal(getAgentContextUsage(AGENT, "chan-1", ROOT)?.used, 1);
      assert.equal(getAgentContextUsage(AGENT, "chan-1", ROOT_2)?.used, 2);
      assert.equal(getAgentContextUsage(AGENT, "chan-1", null)?.used, 3);
      assert.equal(getAgentContextUsage(AGENT, "chan-2", ROOT)?.used, 4);
      assert.equal(getAgentContextUsage(AGENT_2, "chan-1", ROOT)?.used, 5);
      assert.equal(getAgentContextUsage(AGENT_2, "chan-1", null), null);
    });

    it("looks up case-insensitively on agent pubkey and thread root", () => {
      ingestContextUsageEvents(AGENT, [usageEvent()]);
      assert.ok(getAgentContextUsage(AGENT_KEY, "chan-1", ROOT.toUpperCase()));
    });

    it("resolves a message to its thread scope, else the conversation scope", () => {
      ingestContextUsageEvents(AGENT, [
        usageEvent({ seq: 1, threadRootEventId: ROOT, used: 10 }),
      ]);
      assert.equal(resolveMessageContextUsage(AGENT, "chan-1", ROOT)?.used, 10);
      // No thread reading and no conversation reading: hidden.
      assert.equal(resolveMessageContextUsage(AGENT, "chan-1", ROOT_2), null);

      ingestContextUsageEvents(AGENT, [
        usageEvent({ seq: 2, threadRootEventId: null, used: 20 }),
      ]);
      assert.equal(
        resolveMessageContextUsage(AGENT, "chan-1", ROOT_2)?.used,
        20,
      );
      // Thread scope still wins over the conversation scope.
      assert.equal(resolveMessageContextUsage(AGENT, "chan-1", ROOT)?.used, 10);
      assert.equal(resolveMessageContextUsage(AGENT, "chan-9", ROOT), null);
      assert.equal(resolveMessageContextUsage(null, "chan-1", ROOT), null);
    });
  });

  describe("ordering", () => {
    it("keeps the newest reading by timestamp, then seq", () => {
      ingestContextUsageEvents(AGENT, [
        usageEvent({ seq: 5, timestamp: "2026-10-07T00:00:10.000Z", used: 2 }),
      ]);
      // Older timestamp, higher seq: rejected.
      ingestContextUsageEvents(AGENT, [
        usageEvent({ seq: 9, timestamp: "2026-10-07T00:00:05.000Z", used: 1 }),
      ]);
      assert.equal(getAgentContextUsage(AGENT, "chan-1", ROOT)?.used, 2);
      // Same timestamp, lower seq: rejected.
      ingestContextUsageEvents(AGENT, [
        usageEvent({ seq: 4, timestamp: "2026-10-07T00:00:10.000Z", used: 3 }),
      ]);
      assert.equal(getAgentContextUsage(AGENT, "chan-1", ROOT)?.used, 2);
      // Same timestamp, higher seq: accepted.
      ingestContextUsageEvents(AGENT, [
        usageEvent({ seq: 6, timestamp: "2026-10-07T00:00:10.000Z", used: 4 }),
      ]);
      assert.equal(getAgentContextUsage(AGENT, "chan-1", ROOT)?.used, 4);
      // Harness restart: seq resets but timestamp climbs.
      ingestContextUsageEvents(AGENT, [
        usageEvent({ seq: 1, timestamp: "2026-10-07T00:01:00.000Z", used: 5 }),
      ]);
      assert.equal(getAgentContextUsage(AGENT, "chan-1", ROOT)?.used, 5);
    });

    it("is idempotent under replay and notifies only on change", () => {
      let notifications = 0;
      const unsubscribe = subscribeAgentContextUsage(() => {
        notifications += 1;
      });
      const events = [usageEvent({ seq: 1 }), usageEvent({ seq: 2, used: 7 })];
      assert.equal(ingestContextUsageEvents(AGENT, events), true);
      assert.equal(ingestContextUsageEvents(AGENT, events), false);
      assert.equal(notifications, 1);
      assert.equal(getAgentContextUsage(AGENT, "chan-1", ROOT)?.used, 7);
      unsubscribe();
    });

    it("ingests through the observer listener and ignores non-usage events", () => {
      contextUsageObserverListener({
        agentPubkey: AGENT,
        events: [
          usageEvent({ kind: "turn_completed" }),
          usageEvent({ seq: 2, used: 9 }),
        ],
      });
      contextUsageObserverListener(undefined);
      assert.equal(getAgentContextUsage(AGENT, "chan-1", ROOT)?.used, 9);
    });

    it("lists an agent's scopes most recently updated first", () => {
      ingestContextUsageEvents(AGENT, [
        usageEvent({
          seq: 1,
          timestamp: "2026-10-07T00:00:01.000Z",
          threadRootEventId: ROOT,
        }),
        usageEvent({
          seq: 2,
          timestamp: "2026-10-07T00:00:03.000Z",
          threadRootEventId: ROOT_2,
        }),
        usageEvent({
          seq: 3,
          timestamp: "2026-10-07T00:00:02.000Z",
          threadRootEventId: null,
        }),
      ]);
      const first = getAgentContextUsages(AGENT);
      assert.deepEqual(
        first.map((reading) => reading.threadRootEventId),
        [ROOT_2, null, ROOT],
      );
      // Stable reference until the store changes.
      assert.equal(getAgentContextUsages(AGENT), first);
      assert.deepEqual(getAgentContextUsages(AGENT_2), []);
    });
  });

  describe("bound", () => {
    it("evicts the least recently updated scopes past the cap", () => {
      const base = Date.parse("2026-10-07T00:00:00.000Z");
      const events = [];
      for (let index = 0; index < MAX_CONTEXT_USAGE_ENTRIES + 5; index += 1) {
        events.push(
          usageEvent({
            seq: index + 1,
            timestamp: new Date(base + index * 1000).toISOString(),
            threadRootEventId: index.toString(16).padStart(64, "0"),
          }),
        );
      }
      ingestContextUsageEvents(AGENT, events);
      const all = getAgentContextUsages(AGENT);
      assert.equal(all.length, MAX_CONTEXT_USAGE_ENTRIES);
      for (let index = 0; index < 5; index += 1) {
        assert.equal(
          getAgentContextUsage(
            AGENT,
            "chan-1",
            index.toString(16).padStart(64, "0"),
          ),
          null,
        );
      }
      assert.ok(
        getAgentContextUsage(
          AGENT,
          "chan-1",
          (5).toString(16).padStart(64, "0"),
        ),
      );
    });
  });

  describe("persistence", () => {
    it("persists per owner and restores after a restart", () => {
      setContextUsageOwnerScope(OWNER);
      ingestContextUsageEvents(AGENT, [usageEvent({ used: 42 })]);
      const raw = storage.getItem(`buzz-agent-context-usage.v1:${OWNER}`);
      assert.ok(raw);
      assert.equal(JSON.parse(raw).entries.length, 1);

      // Simulated restart: memory cleared, storage retained.
      _testResetAgentContextUsageStore();
      assert.equal(getAgentContextUsage(AGENT, "chan-1", ROOT), null);
      setContextUsageOwnerScope(OWNER);
      const restored = getAgentContextUsage(AGENT, "chan-1", ROOT);
      assert.equal(restored?.used, 42);
      assert.equal(restored?.updatedAt, Date.parse("2026-10-07T00:00:00.000Z"));
    });

    it("merges readings that arrived before the owner resolved", () => {
      setContextUsageOwnerScope(OWNER);
      ingestContextUsageEvents(AGENT, [
        usageEvent({ seq: 1, timestamp: "2026-10-07T00:00:00.000Z", used: 1 }),
        usageEvent({ seq: 2, threadRootEventId: ROOT_2, used: 2 }),
      ]);
      _testResetAgentContextUsageStore();

      // A newer live reading for ROOT lands before identity resolves.
      ingestContextUsageEvents(AGENT, [
        usageEvent({ seq: 1, timestamp: "2026-10-07T00:05:00.000Z", used: 9 }),
      ]);
      setContextUsageOwnerScope(OWNER);
      assert.equal(getAgentContextUsage(AGENT, "chan-1", ROOT)?.used, 9);
      assert.equal(getAgentContextUsage(AGENT, "chan-1", ROOT_2)?.used, 2);
      const persisted = JSON.parse(
        storage.getItem(`buzz-agent-context-usage.v1:${OWNER}`),
      );
      assert.equal(persisted.entries.length, 2);
    });

    it("never shows one owner's readings to another", () => {
      setContextUsageOwnerScope(OWNER);
      ingestContextUsageEvents(AGENT, [usageEvent()]);
      setContextUsageOwnerScope(OWNER_2);
      assert.equal(getAgentContextUsage(AGENT, "chan-1", ROOT), null);
      setContextUsageOwnerScope(OWNER);
      assert.ok(getAgentContextUsage(AGENT, "chan-1", ROOT));
    });

    it("drops malformed persisted entries and survives corrupt JSON", () => {
      storage.setItem(
        `buzz-agent-context-usage.v1:${OWNER}`,
        JSON.stringify({
          entries: [
            { agentPubkey: AGENT_KEY, channelId: "chan-1", used: 1 },
            parseContextUsageEvent(AGENT, usageEvent({ used: 3 })),
          ],
        }),
      );
      setContextUsageOwnerScope(OWNER);
      assert.equal(getAgentContextUsages(AGENT).length, 1);
      assert.equal(getAgentContextUsage(AGENT, "chan-1", ROOT)?.used, 3);

      _testResetAgentContextUsageStore();
      storage.setItem(`buzz-agent-context-usage.v1:${OWNER_2}`, "{not json");
      setContextUsageOwnerScope(OWNER_2);
      assert.deepEqual(getAgentContextUsages(AGENT), []);
    });
  });

  describe("groupContextUsagesByChannel", () => {
    it("splits conversation and thread scopes per channel, keeping order", () => {
      ingestContextUsageEvents(AGENT, [
        usageEvent({
          seq: 1,
          timestamp: "2026-10-07T00:00:01.000Z",
          threadRootEventId: ROOT,
        }),
        usageEvent({
          seq: 2,
          timestamp: "2026-10-07T00:00:02.000Z",
          threadRootEventId: ROOT_2,
        }),
        usageEvent({
          seq: 3,
          timestamp: "2026-10-07T00:00:03.000Z",
          threadRootEventId: null,
        }),
        usageEvent({ seq: 4, channelId: "chan-2", threadRootEventId: null }),
      ]);
      const groups = groupContextUsagesByChannel(getAgentContextUsages(AGENT));
      assert.equal(groups.get("chan-1")?.conversation?.threadRootEventId, null);
      assert.deepEqual(
        groups.get("chan-1")?.threads.map((r) => r.threadRootEventId),
        [ROOT_2, ROOT],
      );
      assert.deepEqual(groups.get("chan-2")?.threads, []);
      assert.ok(groups.get("chan-2")?.conversation);
    });
  });
});
