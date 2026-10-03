import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

// Regression pin for the reachability bug: the first "Default AI" switch lived
// in the header settings menu slot, which `UserProfilePanel` renders only when
// `!isBot` — so for every managed agent it never mounted and nobody could find
// it. The Runtime tab (`ProfileRuntimeTabContent`) cannot be rendered in jsdom
// (its session-panel import chain reads `import.meta.env` at module load), so
// this pins the wiring in source, the same way UserProfileRuntimeContent.test
// does. `ProfileDefaultAiRow.jsdom-test.mjs` covers the row's behaviour.

const read = (file) => readFile(new URL(file, import.meta.url), "utf8");
const [panelSource, sectionsSource, tabsSource, actionsSource] =
  await Promise.all([
    read("./UserProfilePanel.tsx"),
    read("./UserProfilePanelSections.tsx"),
    read("./UserProfilePanelTabs.tsx"),
    read("./UserProfileAgentActions.tsx"),
  ]);

function indexOfOrFail(source, needle, label) {
  const index = source.indexOf(needle);
  assert.notEqual(index, -1, `${label} must contain ${JSON.stringify(needle)}`);
  return index;
}

test("the header settings menu slot is still gated to humans and carries no default-AI switch", () => {
  const slotStart = indexOfOrFail(
    panelSource,
    "const agentSettingsMenu = isBot ? null : (",
    "UserProfilePanel",
  );
  const slotEnd = panelSource.indexOf("/>", slotStart);
  const slotBlock = panelSource.slice(slotStart, slotEnd);
  assert.ok(slotBlock.includes("<UserProfileAgentSettingsMenuSlot"));
  assert.doesNotMatch(
    slotBlock,
    /onToggleDefaultAi/,
    "the slot never mounts for agents; a default-AI prop there is dead",
  );
  assert.doesNotMatch(actionsSource, /Default AI|DefaultAi|isDefaultAi/);
});

test("the panel hands its one toggle handler to the summary view", () => {
  indexOfOrFail(
    panelSource,
    "handleToggleAgentDefaultAi={handleToggleAgentDefaultAi}",
    "UserProfilePanel",
  );
  assert.equal(
    panelSource.match(/handleToggleAgentDefaultAi=\{/g)?.length,
    1,
    "exactly one consumer of the handler",
  );
});

test("the summary view passes the toggle to the Runtime tab only for an eligible agent's owner", () => {
  const passStart = indexOfOrFail(
    sectionsSource,
    "onToggleDefaultAi={",
    "UserProfilePanelSections",
  );
  const passBlock = sectionsSource.slice(passStart, passStart + 320);
  assert.match(passBlock, /isOwner === true/);
  assert.match(passBlock, /isDefaultAiEligible\(managedAgent\)/);
  assert.match(passBlock, /\? handleToggleAgentDefaultAi\s*: undefined/);
  assert.match(
    sectionsSource,
    /defaultAiEnabled=\{managedAgent\?\.isDefaultAi \?\? false\}/,
  );
});

test("the Runtime tab mounts the row in the Activity group directly under Start on launch", () => {
  const activity = indexOfOrFail(
    tabsSource,
    'testId="user-profile-runtime-activity-section"',
    "UserProfilePanelTabs",
  );
  const startOnLaunchRow = indexOfOrFail(
    tabsSource,
    "data-testid={startOnLaunchField.testId}",
    "UserProfilePanelTabs",
  );
  const defaultAiRow = indexOfOrFail(
    tabsSource,
    "<ProfileDefaultAiRow",
    "UserProfilePanelTabs",
  );
  const harnessLog = indexOfOrFail(
    tabsSource,
    'label="Harness log"',
    "UserProfilePanelTabs",
  );
  const configuration = indexOfOrFail(
    tabsSource,
    'testId="user-profile-agent-configuration-section"',
    "UserProfilePanelTabs",
  );
  assert.ok(activity < startOnLaunchRow, "Start on launch is an Activity row");
  assert.ok(
    startOnLaunchRow < defaultAiRow && defaultAiRow < harnessLog,
    "Default AI sits right after Start on launch, before the harness log row",
  );
  assert.ok(defaultAiRow < configuration, "and inside the Activity group");
  // Mounted only when the owner handler exists; absent rows must not leave an
  // empty Activity group behind either.
  assert.match(tabsSource, /\{onToggleDefaultAi \? \(\s*<ProfileDefaultAiRow/);
  assert.match(tabsSource, /canToggleDefaultAi \|\|\s*showDiagnosticsIngress/);
});
