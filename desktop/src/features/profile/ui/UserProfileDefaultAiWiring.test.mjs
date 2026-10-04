import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

// Regression pin for the reachability bug: the first "Default AI" switch lived
// in the header settings menu slot, which `UserProfilePanel` renders only when
// `!isBot` — so for every managed agent it never mounted and nobody could find
// it. The gates themselves are pure and unit-tested in
// `../lib/profileRuntimeGates.test.mjs`; `ProfileDefaultAiRow.jsdom-test.mjs`
// covers the row. The Runtime tab module cannot be rendered in jsdom (its
// session-panel import chain reads `import.meta.env` at module load), so what
// remains here are token-level pins, the same way
// UserProfileRuntimeContent.test does: no toggle on the dead slot, Sections
// routes through the shared gates, and the row is mounted under the Status
// row.

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
  indexOfOrFail(
    panelSource,
    "const agentSettingsMenu = isBot ? null : (",
    "UserProfilePanel",
  );
  // The panel's only default-AI wire is the summary-view prop; the slot and
  // the menu component know nothing about the star.
  assert.equal(panelSource.includes("onToggleDefaultAi"), false);
  assert.doesNotMatch(actionsSource, /Default AI|DefaultAi|isDefaultAi/);
});

test("the summary view decides the tab and the toggle through the shared gates", () => {
  indexOfOrFail(
    sectionsSource,
    "const showRuntimeTab = shouldShowRuntimeTab({",
    "UserProfilePanelSections",
  );
  indexOfOrFail(
    sectionsSource,
    "onToggleDefaultAi={defaultAiToggleFor({",
    "UserProfilePanelSections",
  );
  // No second, inline copy of either gate.
  assert.equal(sectionsSource.includes("isDefaultAiEligible("), false);
});

test("the Runtime tab mounts the row in the Activity group directly under the Status row", () => {
  const activity = indexOfOrFail(
    tabsSource,
    'testId="user-profile-runtime-activity-section"',
    "UserProfilePanelTabs",
  );
  const statusRow = indexOfOrFail(
    tabsSource,
    "fields={statusDiagnosticsFields}",
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
  assert.ok(activity < statusRow, "Status is an Activity row");
  assert.ok(
    statusRow < defaultAiRow && defaultAiRow < harnessLog,
    "Default AI sits right after the Status row, before the harness log row",
  );
  assert.ok(defaultAiRow < configuration, "and inside the Activity group");
});
