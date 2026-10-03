import assert from "node:assert/strict";
import test from "node:test";

import { joinAgentsAfterCreate } from "./joinAgentsAfterCreate.ts";

const CHANNEL = "11111111-1111-4111-8111-111111111111";

function recordingDeps() {
  const events = [];
  let releaseAttach;
  const deps = {
    attachDefaultAi: (channelId) => {
      events.push(`attach:start:${channelId}`);
      return new Promise((resolve) => {
        releaseAttach = () => {
          events.push("attach:done");
          resolve();
        };
      });
    },
    applyAgents: async (templateId, channelId) => {
      events.push(`apply:${templateId ?? "none"}:${channelId}`);
    },
  };
  return { deps, events, releaseAttach: () => releaseAttach() };
}

test("the default AI attach resolves before template agents are applied", async () => {
  const { deps, events, releaseAttach } = recordingDeps();

  const run = joinAgentsAfterCreate(deps, {
    channelId: CHANNEL,
    templateId: "tpl-1",
    addDefaultAi: true,
  });
  await Promise.resolve();
  assert.deepEqual(events, [`attach:start:${CHANNEL}`]);

  releaseAttach();
  await run;
  assert.deepEqual(events, [
    `attach:start:${CHANNEL}`,
    "attach:done",
    `apply:tpl-1:${CHANNEL}`,
  ]);
});

test("template agents still apply when the default AI is not requested", async () => {
  for (const addDefaultAi of [false, undefined]) {
    const { deps, events } = recordingDeps();
    await joinAgentsAfterCreate(deps, {
      channelId: CHANNEL,
      templateId: "tpl-1",
      addDefaultAi,
    });
    assert.deepEqual(events, [`apply:tpl-1:${CHANNEL}`]);
  }
});

test("without a template the default AI still joins and applyAgents gets undefined", async () => {
  const { deps, events, releaseAttach } = recordingDeps();
  const run = joinAgentsAfterCreate(deps, {
    channelId: CHANNEL,
    addDefaultAi: true,
  });
  await Promise.resolve();
  releaseAttach();
  await run;
  assert.deepEqual(events, [
    `attach:start:${CHANNEL}`,
    "attach:done",
    `apply:none:${CHANNEL}`,
  ]);
});
