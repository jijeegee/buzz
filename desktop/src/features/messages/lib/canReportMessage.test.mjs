import assert from "node:assert/strict";
import test from "node:test";

import { KIND_HUDDLE_STARTED } from "../../../shared/constants/kinds.ts";
import { canReportMessage, isModeratableMessage } from "./canReportMessage.ts";

const ME = "a".repeat(64);
const OTHER = "b".repeat(64);
const AGENT = "c".repeat(64);

function message(overrides = {}) {
  return {
    author: "Someone",
    body: "hi",
    depth: 0,
    id: "e".repeat(64),
    pubkey: OTHER,
    time: "9:00",
    ...overrides,
  };
}

const noAgents = () => false;

test("another person's delivered message is reportable", () => {
  assert.equal(
    canReportMessage({
      currentPubkey: ME,
      isAgentPubkey: noAgents,
      message: message(),
    }),
    true,
  );
});

test("my own message is never reportable", () => {
  assert.equal(
    canReportMessage({
      currentPubkey: ME,
      isAgentPubkey: noAgents,
      message: message({ pubkey: ME.toUpperCase() }),
    }),
    false,
  );
  // The row's own-message flag alone also hides it (identity not loaded yet).
  assert.equal(
    canReportMessage({
      currentPubkey: undefined,
      isAgentPubkey: noAgents,
      message: message({ accent: true }),
    }),
    false,
  );
});

test("agent messages are never reportable", () => {
  for (const overrides of [{ isAgent: true }, { role: "bot" }]) {
    assert.equal(
      canReportMessage({
        currentPubkey: ME,
        isAgentPubkey: noAgents,
        message: message(overrides),
      }),
      false,
      JSON.stringify(overrides),
    );
  }
  assert.equal(
    canReportMessage({
      currentPubkey: ME,
      isAgentPubkey: (pubkey) => pubkey === AGENT,
      message: message({ pubkey: AGENT }),
    }),
    false,
  );
});

test("pending, huddle and authorless rows are neither reportable nor moderatable", () => {
  for (const overrides of [
    { pending: true },
    { kind: KIND_HUDDLE_STARTED },
    { pubkey: undefined },
  ]) {
    const row = message(overrides);
    assert.equal(isModeratableMessage(row), false);
    assert.equal(
      canReportMessage({
        currentPubkey: ME,
        isAgentPubkey: noAgents,
        message: row,
      }),
      false,
    );
  }
});

test("own and agent messages stay moderatable for admins", () => {
  assert.equal(isModeratableMessage(message({ isAgent: true })), true);
  assert.equal(isModeratableMessage(message({ accent: true })), true);
});
