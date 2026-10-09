import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import test from "node:test";

import React from "react";
import { renderToStaticMarkup } from "react-dom/server";

import { LifecycleActivity } from "./activityRenderClasses/LifecycleActivity.tsx";
import { ToolDetailBlocks } from "./AgentSessionToolItem/ToolDetailBlocks.tsx";
import {
  describeObserverGap,
  isObserverSummaryView,
} from "./agentSessionObserverSummary.ts";
import { buildTranscript } from "./agentSessionTranscript.ts";

// Shared with mobile (transcript_builder_test.dart): real buzz-acp captures
// plus synthetic summary-tier cases, each with the rows both apps must show.
const FIXTURE_DIR = new URL(
  "../../../../../test-fixtures/observer-summary/",
  import.meta.url,
);
const fixtures = readdirSync(FIXTURE_DIR)
  .filter((name) => name.endsWith(".json"))
  .sort()
  .map((name) => ({
    name,
    ...JSON.parse(readFileSync(new URL(name, FIXTURE_DIR), "utf8")),
  }));

// The fixture row projection: assistant messages, tools (summarised tools
// display their argument preview in place of arguments) and gap lines.
function projectRows(items) {
  return items.flatMap((item) => {
    if (item.type === "message" && item.role === "assistant") {
      return [{ type: "message", role: "assistant", text: item.text }];
    }
    if (item.type === "tool") {
      return [
        {
          type: "tool",
          title: item.title,
          status: item.status,
          args: item.argsPreview ?? item.args,
          result: item.result,
        },
      ];
    }
    if (item.type === "lifecycle" && item.acpSource === "observer_gap") {
      return [{ type: "gap", text: item.title }];
    }
    return [];
  });
}

// Huge captured results are pinned by prefix and length.
function assertRowsMatch(actual, expected, name) {
  assert.equal(actual.length, expected.length, `${name}: row count`);
  expected.forEach((row, index) => {
    const got = actual[index];
    if (row.result && typeof row.result === "object") {
      assert.ok(
        got.result.startsWith(row.result.startsWith),
        `${name}: row ${index} result prefix`,
      );
      assert.equal(
        got.result.length,
        row.result.length,
        `${name}: row ${index} result length`,
      );
      assert.deepEqual(
        { ...got, result: null },
        { ...row, result: null },
        `${name}: row ${index}`,
      );
    } else {
      assert.deepEqual(got, row, `${name}: row ${index}`);
    }
  });
}

assert.ok(fixtures.length >= 5, "observer summary fixtures are present");

for (const fixture of fixtures) {
  test(`observer summary fixture ${fixture.name}: transcript rows`, () => {
    assertRowsMatch(
      projectRows(buildTranscript(fixture.events)),
      fixture.expected.rows,
      fixture.name,
    );
  });

  test(`observer summary fixture ${fixture.name}: summary badge`, () => {
    assert.equal(
      isObserverSummaryView(fixture.events),
      fixture.expected.summaryView,
    );
  });
}

test("describeObserverGap covers singular, plural, ellipsis and skipped updates", () => {
  const cases = [
    [
      {
        folded_events: 30,
        folded_tools: 12,
        tool_names: ["Read", "Bash", "Edit"],
      },
      "12 tools ran in between (Read, Bash, Edit…)",
    ],
    [
      { folded_events: 2, folded_tools: 1, tool_names: ["Read"] },
      "1 tool ran in between (Read)",
    ],
    [
      { folded_events: 9, folded_tools: 2, tool_names: ["Read", "Bash"] },
      "2 tools ran in between (Read, Bash)",
    ],
    [
      { folded_events: 4, folded_tools: 3, tool_names: [] },
      "3 tools ran in between",
    ],
    [
      { folded_events: 3, folded_tools: 0, tool_names: [] },
      "3 updates skipped",
    ],
    [{ folded_events: 1, folded_tools: 0, tool_names: [] }, "1 update skipped"],
    [{}, "0 updates skipped"],
  ];
  for (const [payload, text] of cases) {
    assert.equal(describeObserverGap(payload), text);
  }
});

test("isObserverSummaryView follows the latest summarisable event", () => {
  const acp = (detail) => ({ kind: "acp_read", detail });
  const cases = [
    [[], false],
    [[acp(undefined)], false],
    [[acp("free")], true],
    [[acp("standard")], true],
    [[acp("premium")], false],
    [[{ kind: "observer_gap", detail: "free" }], true],
    // Lifecycle events never carry detail and must not reset the view.
    [[acp("free"), { kind: "turn_completed" }], true],
    // An upgrade to full detail mid-session drops the badge, and vice versa.
    [[acp("free"), acp(undefined)], false],
    [[acp(undefined), acp("standard")], true],
  ];
  for (const [events, expected] of cases) {
    assert.equal(isObserverSummaryView(events), expected);
  }
});

const turnEvent = (seq, kind, turnId, payload = {}) => ({
  seq,
  timestamp: `2026-10-10T00:00:${String(seq).padStart(2, "0")}Z`,
  kind,
  agentIndex: 0,
  channelId: "chan-1",
  sessionId: "sess-1",
  turnId,
  payload,
});
const toolEvent = (seq, turnId, update, detail = "free") => ({
  ...turnEvent(seq, "acp_read", turnId, {
    method: "session/update",
    params: { sessionId: "sess-1", update },
  }),
  detail,
});

test("turn_error alone fails the turn's running tools", () => {
  const items = buildTranscript([
    toolEvent(1, "t1", {
      sessionUpdate: "tool_call",
      toolCallId: "x",
      title: "Build",
      status: "in_progress",
    }),
    turnEvent(2, "turn_error", "t1", { outcome: "error", error: "boom" }),
  ]);
  const tool = items.find((item) => item.type === "tool");
  assert.equal(tool.status, "failed");
  assert.equal(tool.completedAt, "2026-10-10T00:00:02Z");
});

test("turn end leaves premium tool items without summary fields", () => {
  const items = buildTranscript([
    toolEvent(
      1,
      "t1",
      {
        sessionUpdate: "tool_call",
        toolCallId: "x",
        title: "Read file",
        status: "completed",
        rawInput: { path: "a.txt" },
      },
      null,
    ),
    turnEvent(2, "turn_completed", "t1"),
  ]);
  const tool = items.find((item) => item.type === "tool");
  assert.deepEqual(tool.args, { path: "a.txt" });
  assert.equal("argsPreview" in tool, false);
  assert.equal("closedByTurnEnd" in tool, false);
});

test("ToolDetailBlocks shows a summary preview as parameters with no result placeholder", () => {
  const html = renderToStaticMarkup(
    React.createElement(ToolDetailBlocks, {
      args: {},
      argsPreview: '{"command":"cargo te…',
      fileEditDiff: null,
      fileReadContent: null,
      hasArgs: true,
      hasResult: false,
      imagePreview: null,
      isError: false,
      result: "",
      shellCommand: null,
    }),
  );
  assert.ok(html.includes("Parameters"));
  assert.ok(html.includes("{&quot;command&quot;:&quot;cargo te…"));
  assert.ok(!html.includes("Result"));
  assert.ok(!html.includes("Waiting for tool details"));
});

test("gap lines render as one plain row even when a tool name says error", () => {
  const html = renderToStaticMarkup(
    React.createElement(LifecycleActivity, {
      agentAvatarUrl: null,
      agentName: "Agent",
      agentPubkey: "pubkey123",
      item: {
        id: "gap:chan-1:4",
        type: "lifecycle",
        renderClass: "status",
        title: "2 tools ran in between (get_errors)",
        text: "",
        timestamp: "2026-10-10T00:00:04Z",
        acpSource: "observer_gap",
      },
    }),
  );
  assert.ok(html.includes("2 tools ran in between (get_errors)"));
  assert.ok(!html.includes("text-destructive"));
});
