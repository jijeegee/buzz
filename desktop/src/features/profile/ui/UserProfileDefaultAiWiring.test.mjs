import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

// The routing agent (the default-AI star) has exactly one edit location: the
// picker in the Agents page "Channel routing" card. The profile Runtime tab
// used to carry a "Default AI" switch; it was removed so the star cannot be
// moved behind the mode's back (AGENTS.md rules 20 and 23). The Runtime tab
// module cannot be rendered in jsdom (its session-panel import chain reads
// `import.meta.env` at module load), so these are token-level pins, the same
// way UserProfileRuntimeContent.test does.

const read = (file) => readFile(new URL(file, import.meta.url), "utf8");
const [panelSource, sectionsSource, tabsSource, actionsSource, cardSource] =
  await Promise.all([
    read("./UserProfilePanel.tsx"),
    read("./UserProfilePanelSections.tsx"),
    read("./UserProfilePanelTabs.tsx"),
    read("./UserProfileAgentActions.tsx"),
    read("../../agents/ui/routing/ChannelRoutingCard.tsx"),
  ]);

const STAR_WRITERS =
  /Default AI|DefaultAi|setDefaultManagedAgent|set_default_managed_agent/;

test("no profile source can move the routing agent star", () => {
  for (const [label, source] of [
    ["UserProfilePanel", panelSource],
    ["UserProfilePanelSections", sectionsSource],
    ["UserProfilePanelTabs", tabsSource],
    ["UserProfileAgentActions", actionsSource],
  ]) {
    assert.doesNotMatch(source, STAR_WRITERS, label);
  }
});

test("the Channel routing card is where the star is chosen, through set_channel_routing", () => {
  assert.match(cardSource, /useSetChannelRoutingMutation/);
  assert.doesNotMatch(cardSource, /setDefaultManagedAgent/);
});
