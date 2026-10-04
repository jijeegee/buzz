import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const panelSectionsSource = await readFile(
  new URL("./UserProfilePanelSections.tsx", import.meta.url),
  "utf8",
);
const panelTabsSource = await readFile(
  new URL("./UserProfilePanelTabs.tsx", import.meta.url),
  "utf8",
);
const panelSource = await readFile(
  new URL("./UserProfilePanel.tsx", import.meta.url),
  "utf8",
);
const panelFieldsSource = await readFile(
  new URL("./UserProfilePanelFields.tsx", import.meta.url),
  "utf8",
);
const agentActionsSource = await readFile(
  new URL("./UserProfileAgentActions.tsx", import.meta.url),
  "utf8",
);

test("profile runtime surfaces never synthesize preview agent data", () => {
  for (const forbiddenPattern of [
    /UserProfileRuntimePreview/,
    /fillRuntimePreview/,
    /showRuntimePreview/,
    /UserProfileConfigPreview/,
    /import\.meta\.env\.DEV/,
  ]) {
    assert.doesNotMatch(panelSectionsSource, forbiddenPattern);
  }
});

test("Start on launch is a read-only Runtime row; its toggle lives in the Agents page card menu", () => {
  // The flag is still reported as an effective value (rule 13)...
  assert.match(panelFieldsSource, /label: "Start on launch"/);
  assert.match(panelFieldsSource, /testId: "user-profile-start-on-launch"/);
  // ...but no profile surface turns it into a switch, owns its handler, or
  // revives the unreachable header-menu Auto-start item (agents AGENTS.md 22).
  for (const source of [
    panelSource,
    panelSectionsSource,
    panelTabsSource,
    agentActionsSource,
  ]) {
    assert.doesNotMatch(
      source,
      /startOnLaunch|StartOnLaunch|Start on launch|AutoStart|Auto-start|start_on_app_launch/,
    );
  }
});

test("runtime rows do not add interactive preview-only controls", () => {
  for (const forbiddenPattern of [
    /previewStartOnLaunchEnabled/,
    /isRuntimePreview/,
    /showPreviewHarnessLog/,
    /diagnostics-ingress-preview/,
  ]) {
    assert.doesNotMatch(panelTabsSource, forbiddenPattern);
  }
});
