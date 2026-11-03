import assert from "node:assert/strict";
import test from "node:test";
import {
  classifySubId,
  getOpenSubscriptions,
  getReqLog,
  recordClose,
  recordRelayClosed,
  recordReq,
  recordSocketClosed,
  recordSocketOpened,
  resetReqLog,
  subscriptionSignature,
} from "./e2eBridgeReqLog.ts";

const openIds = () => getOpenSubscriptions().map((sub) => sub.subId);

test("open set follows REQ, re-REQ, CLOSE, relay CLOSED and socket teardown", () => {
  resetReqLog();
  recordSocketOpened(1);
  recordReq(1, "live-x", [{ kinds: [9] }]);
  recordReq(1, "live-x", [{ kinds: [7] }]);
  assert.deepEqual(openIds(), ["live-x"]);
  assert.deepEqual(getOpenSubscriptions()[0].filters, [{ kinds: [7] }]);

  recordReq(1, "history-y", [{ kinds: [9], limit: 50 }]);
  recordClose(1, "history-y");
  recordClose(1, "live-x");
  assert.deepEqual(openIds(), []);

  recordReq(1, "live-z", [{ kinds: [9] }]);
  recordRelayClosed(1, "live-z", "rate-limited");
  assert.deepEqual(openIds(), []);

  recordReq(1, "live-a", [{ kinds: [9] }]);
  recordReq(2, "live-a", [{ kinds: [9] }]);
  recordSocketClosed(1);
  assert.deepEqual(
    getOpenSubscriptions().map((sub) => `${sub.socketId}:${sub.subId}`),
    ["2:live-a"],
  );
  assert.equal(getReqLog().filter((entry) => entry.type === "REQ").length, 6);
});

test("a non-live id left open is reported as unclassified, not dropped", () => {
  resetReqLog();
  recordReq(1, "w-mem", [{ kinds: [39002] }]);
  recordReq(1, "first-q", [{ kinds: [0] }]);
  recordReq(1, "observer-history-r", [{ kinds: [24200] }]);
  assert.deepEqual(
    getOpenSubscriptions().map((sub) => sub.subIdClass),
    ["unclassified", "oneShot", "oneShot"],
  );
  assert.equal(classifySubId("live-1"), "live");
});

const ROOM = "aaaaaaaa-0000-4000-8000-000000000001";
const PK = "b".repeat(64);

test("signatures strip ids and timestamps but keep kinds, limit and list sizes", () => {
  assert.equal(
    subscriptionSignature([
      { "#h": [ROOM, ROOM], kinds: [9, 7], since: 123, limit: 0 },
      { authors: [PK], kinds: [0] },
    ]),
    "#h:2|kinds:[7,9]|limit:0|since + authors:1|kinds:[0]",
  );
});

test("signatures keep non-id tag values and fold the presence author set", () => {
  assert.equal(
    subscriptionSignature([
      { kinds: [30315], "#d": ["general"], limit: 0 },
      { kinds: [20001], authors: [PK, PK, PK], limit: 0 },
      { kinds: [30030], "#d": ["kuzz:custom-emoji", PK], limit: 0 },
    ]),
    "#d:[general]|kinds:[30315]|limit:0 + #d:[kuzz:custom-emoji,*]|kinds:[30030]|limit:0 + authors:+|kinds:[20001]|limit:0",
  );
  // Only a presence-only filter folds its authors.
  assert.equal(
    subscriptionSignature([{ kinds: [0, 20001], authors: [PK], limit: 1 }]),
    "authors:1|kinds:[0,20001]|limit:1",
  );
});
