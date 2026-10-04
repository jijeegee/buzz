import assert from "node:assert/strict";
import test from "node:test";

import { settingsNavGroups } from "./SettingsView.tsx";

test("relay-admin is wired into the Communities nav group", () => {
  const communitiesGroup = settingsNavGroups.find(
    (g) => g.label === "Communities",
  );
  assert.ok(
    communitiesGroup,
    "Communities group must exist in settingsNavGroups",
  );
  assert.ok(
    communitiesGroup.sections.includes("relay-admin"),
    `expected "relay-admin" in Communities group sections, got: ${JSON.stringify(communitiesGroup.sections)}`,
  );
});

test("relay-admin follows community-members in the Communities nav group", () => {
  const communitiesGroup = settingsNavGroups.find(
    (g) => g.label === "Communities",
  );
  assert.ok(
    communitiesGroup,
    "Communities group must exist in settingsNavGroups",
  );
  const membersIndex = communitiesGroup.sections.indexOf("community-members");
  const relayAdminIndex = communitiesGroup.sections.indexOf("relay-admin");
  assert.ok(membersIndex !== -1, "community-members must be present");
  assert.ok(
    relayAdminIndex > membersIndex,
    `expected "relay-admin" after "community-members", got: ${JSON.stringify(communitiesGroup.sections)}`,
  );
});

test("models sits in the App nav group directly before agents", () => {
  const appGroup = settingsNavGroups.find((g) => g.label === "App");
  assert.ok(appGroup, "App group must exist in settingsNavGroups");
  const modelsIndex = appGroup.sections.indexOf("models");
  const agentsIndex = appGroup.sections.indexOf("agents");
  assert.ok(modelsIndex !== -1, "models must be present in the App group");
  assert.equal(
    agentsIndex,
    modelsIndex + 1,
    `expected "agents" immediately after "models", got: ${JSON.stringify(appGroup.sections)}`,
  );
  for (const group of settingsNavGroups) {
    if (group.label !== "App") {
      assert.ok(
        !group.sections.includes("models"),
        `"models" must not also appear in the "${group.label}" group`,
      );
    }
  }
});

test("the removed admin-console id is not wired into any nav group", () => {
  for (const group of settingsNavGroups) {
    assert.ok(
      !group.sections.includes("admin-console"),
      `"admin-console" must not appear in the "${group.label}" group`,
    );
  }
});
