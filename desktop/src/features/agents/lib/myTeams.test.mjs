/**
 * Teams (kind:30182): groups of the owner's real agents. Reading keeps the
 * newest event per team; saving and deleting publish one event and update
 * the cached list. The migration turns old team presets' agents into teams.
 */

import assert from "node:assert/strict";
import { test } from "node:test";

import { QueryClient } from "@tanstack/react-query";

import {
  deleteMyTeam,
  isMyTeamDeletion,
  MY_TEAM_MAX_AGENTS,
  myTeamContent,
  myTeamsFromEvents,
  myTeamsQueryKey,
  saveMyTeam,
} from "./myTeams.ts";
import { teamsFromTeamPresets } from "./teamPresetMigration.ts";

const OWNER = "c".repeat(64);
const OTHER = "d".repeat(64);
const A = "a".repeat(64);
const B = "b".repeat(64);

function teamEvent(d, content, { pubkey = OWNER, createdAt = 100 } = {}) {
  return {
    id: `${d}-${createdAt}`,
    pubkey,
    created_at: createdAt,
    kind: 30182,
    tags: [["d", d]],
    content: typeof content === "string" ? content : JSON.stringify(content),
    sig: "0".repeat(128),
  };
}

test("reads the newest version of each team, only the owner's", () => {
  const teams = myTeamsFromEvents(
    [
      teamEvent(
        "t1",
        { v: 1, name: "Old", agent_pubkeys: [A] },
        { createdAt: 1 },
      ),
      teamEvent("t1", {
        v: 1,
        name: " Build ",
        description: " ships ",
        agent_pubkeys: [B, A.toUpperCase(), B, "nope"],
      }),
      teamEvent("t2", { v: 1, name: "Docs", agent_pubkeys: [] }),
      teamEvent(
        "t3",
        { v: 1, name: "Theirs", agent_pubkeys: [] },
        { pubkey: OTHER },
      ),
      teamEvent("t4", "not json"),
      teamEvent("t5", { v: 1, name: " ", agent_pubkeys: [] }),
    ],
    OWNER,
  );
  assert.deepEqual(
    teams.map((team) => team.name),
    ["Build", "Docs"],
  );
  assert.equal(teams[0].description, "ships");
  // Lowercased, deduplicated, invalid dropped, order kept.
  assert.deepEqual(teams[0].agentPubkeys, [B, A]);
  assert.equal(teams[1].description, null);
});

test("content has the version, trimmed name and unique pubkeys", () => {
  assert.deepEqual(
    JSON.parse(
      myTeamContent({
        name: " Build ",
        description: "",
        agentPubkeys: [A, A, B],
      }),
    ),
    { v: 1, name: "Build", agent_pubkeys: [A, B] },
  );
});

function deps(published) {
  return {
    sign: async (input) => ({
      id: "e",
      pubkey: OWNER,
      created_at: input.createdAt,
      kind: input.kind,
      tags: input.tags,
      content: input.content,
      sig: "0".repeat(128),
    }),
    publish: async (event) => {
      published.push(event);
    },
    now: () => 1_000_000,
    newId: () => "new-team-id",
  };
}

test("saving publishes a team event and caches it; an edit supersedes", async () => {
  const client = new QueryClient();
  const published = [];
  const created = await saveMyTeam(
    client,
    OWNER,
    { name: "Build", agentPubkeys: [A] },
    deps(published),
  );
  assert.equal(created.id, "new-team-id");
  assert.equal(published[0].kind, 30182);
  assert.deepEqual(published[0].tags, [["d", "new-team-id"]]);
  assert.deepEqual(
    client.getQueryData(myTeamsQueryKey(OWNER)).map((team) => team.name),
    ["Build"],
  );

  const edited = await saveMyTeam(
    client,
    OWNER,
    { id: created.id, name: "Build crew", agentPubkeys: [A, B] },
    deps(published),
  );
  // Same second: still newer than the cached version.
  assert.equal(edited.createdAt, created.createdAt + 1);
  assert.deepEqual(
    client
      .getQueryData(myTeamsQueryKey(OWNER))
      .map((team) => team.agentPubkeys),
    [[A, B]],
  );
});

test("saving refuses a nameless or oversized team", async () => {
  const client = new QueryClient();
  await assert.rejects(
    saveMyTeam(client, OWNER, { name: " ", agentPubkeys: [] }, deps([])),
    /needs a name/,
  );
  const many = Array.from({ length: MY_TEAM_MAX_AGENTS + 1 }, (_, i) =>
    i.toString(16).padStart(64, "0"),
  );
  await assert.rejects(
    saveMyTeam(client, OWNER, { name: "Big", agentPubkeys: many }, deps([])),
    /up to 64/,
  );
  // Characters, not bytes: 200 Korean characters fit, 201 do not.
  await saveMyTeam(
    client,
    OWNER,
    { name: "팀".repeat(200), agentPubkeys: [] },
    deps([]),
  );
  await assert.rejects(
    saveMyTeam(
      client,
      OWNER,
      { name: "팀".repeat(201), agentPubkeys: [] },
      deps([]),
    ),
    /up to 200 characters/,
  );
});

test("only the owner's team deletions count as team deletions", () => {
  const deletion = (pubkey, a) => ({
    id: "x",
    pubkey,
    created_at: 1,
    kind: 5,
    tags: [["a", a]],
    content: "",
    sig: "0".repeat(128),
  });
  assert.equal(
    isMyTeamDeletion(deletion(OWNER, `30182:${OWNER}:t1`), OWNER),
    true,
  );
  assert.equal(
    isMyTeamDeletion(deletion(OWNER, `30176:${OWNER}:t1`), OWNER),
    false,
  );
  assert.equal(
    isMyTeamDeletion(deletion(OWNER, `30182:${OTHER}:t1`), OWNER),
    false,
  );
});

test("deleting publishes a coordinate deletion and drops it from the cache", async () => {
  const client = new QueryClient();
  client.setQueryData(myTeamsQueryKey(OWNER), [
    {
      id: "t1",
      name: "Build",
      description: null,
      agentPubkeys: [],
      createdAt: 5,
    },
  ]);
  const published = [];
  await deleteMyTeam(
    client,
    OWNER,
    { id: "t1", name: "Build", createdAt: 5 },
    deps(published),
  );
  assert.equal(published[0].kind, 5);
  assert.deepEqual(published[0].tags, [["a", `30182:${OWNER}:t1`]]);
  assert.deepEqual(client.getQueryData(myTeamsQueryKey(OWNER)), []);
});

test("migration: an old team's agents become a team of the same name", () => {
  const preset = (overrides) => ({
    id: "p1",
    name: "Welcome",
    description: "Hi",
    isBuiltin: false,
    personaIds: [],
    ...overrides,
  });
  const agent = (pubkey, teamId) => ({ pubkey, teamId });
  const inputs = teamsFromTeamPresets(
    [
      preset({}),
      preset({ id: "p2", name: "Unused" }),
      preset({
        id: "builtin-team:welcome",
        name: "Built in",
        description: null,
        isBuiltin: true,
      }),
    ],
    [
      agent(B, "p1"),
      agent(A.toUpperCase(), "p1"),
      agent("e".repeat(64), null),
      // Built-in team presets made agents too.
      agent("f".repeat(64), "builtin-team:welcome"),
    ],
    [],
  );
  assert.deepEqual(inputs, [
    { name: "Welcome", description: "Hi", agentPubkeys: [A, B] },
    { name: "Built in", description: "", agentPubkeys: ["f".repeat(64)] },
  ]);

  // Already made (another run or computer): skipped.
  assert.deepEqual(
    teamsFromTeamPresets(
      [preset({})],
      [agent(A, "p1"), agent(B, "p1")],
      [
        {
          id: "x",
          name: "Welcome",
          agentPubkeys: [B, A],
          description: null,
          createdAt: 1,
        },
      ],
    ),
    [],
  );
});
