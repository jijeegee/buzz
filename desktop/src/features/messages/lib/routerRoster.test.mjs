import assert from "node:assert/strict";
import test from "node:test";

import {
  buildRouterRoster,
  MAX_ROUTER_DESCRIPTION_CHARS,
  MAX_ROUTER_HUMANS,
  MAX_ROUTER_ROSTER,
  routerHumanNames,
  SYSTEM_PROMPT_PREFIX_CHARS,
} from "./routerRoster.ts";

const pk = (n) => n.toString(16).padStart(2, "0").repeat(32);
const ME = pk(0xee);
const agent = (n, label) => ({ pubkey: pk(n), label, isAgent: true });
const human = (n, label) => ({ pubkey: pk(n), label, isAgent: false });

function build(overrides = {}) {
  return buildRouterRoster({
    aboutByPubkey: new Map(),
    identities: [],
    managedAgents: [],
    memberPubkeys: new Set(),
    personas: [],
    selfPubkey: ME,
    ...overrides,
  });
}

test("description priority: persona description > kind:0 about > own system prompt > none", () => {
  const roster = build({
    identities: [
      agent(1, "Coder"),
      agent(2, "Translator"),
      agent(3, "Scribe"),
      agent(4, "Remote"),
      agent(5, "Bare"),
    ],
    memberPubkeys: new Set([pk(1), pk(2), pk(3), pk(4), pk(5)]),
    managedAgents: [
      { pubkey: pk(1), personaId: "p-coder", systemPrompt: null },
      { pubkey: pk(2), personaId: "p-trans", systemPrompt: null },
      {
        pubkey: pk(3),
        personaId: null,
        systemPrompt: "You take notes. ".repeat(30),
      },
    ],
    personas: [
      {
        id: "p-coder",
        description: " Writes code. ",
        systemPrompt: "secret A",
      },
      { id: "p-trans", description: "   ", systemPrompt: "Translate Korean." },
    ],
    aboutByPubkey: new Map([
      [pk(1), "about loses to the persona description"],
      [pk(2), "Translates between Korean and English."],
      [pk(4), "Looks things up on the web."],
    ]),
  });
  assert.deepEqual(
    roster.map((entry) => [entry.name, entry.description]),
    [
      ["Coder", "Writes code."],
      ["Translator", "Translates between Korean and English."],
      [
        "Scribe",
        "You take notes. "
          .repeat(30)
          .trim()
          .slice(0, SYSTEM_PROMPT_PREFIX_CHARS),
      ],
      ["Remote", "Looks things up on the web."],
      ["Bare", null],
    ],
  );
});

test("someone else's agent never contributes a system prompt", () => {
  const roster = build({
    identities: [agent(4, "Remote")],
    memberPubkeys: new Set([pk(4)]),
  });
  assert.equal(roster[0].description, null);
});

test("only agent members other than me, once each, sorted by pubkey", () => {
  const roster = build({
    identities: [
      agent(9, "Nine"),
      agent(2, "Two"),
      agent(2, "Two again"),
      agent(7, "Not a member"),
      human(3, "Jiho"),
      agent(0xee, "Me as agent?"),
      agent(5, "   "),
    ],
    memberPubkeys: new Set([pk(9), pk(2), pk(3), pk(0xee), pk(5)]),
  });
  assert.deepEqual(
    roster.map((entry) => entry.name),
    ["Two", "Nine"],
  );
});

test("caps: roster size and description length", () => {
  const identities = Array.from({ length: 40 }, (_, i) =>
    agent(i + 1, `A${i}`),
  );
  const roster = build({
    identities,
    memberPubkeys: new Set(identities.map((identity) => identity.pubkey)),
    aboutByPubkey: new Map(
      identities.map((identity) => [identity.pubkey, "x".repeat(500)]),
    ),
  });
  assert.equal(roster.length, MAX_ROUTER_ROSTER);
  assert.ok(
    roster.every(
      (entry) => entry.description.length === MAX_ROUTER_DESCRIPTION_CHARS,
    ),
  );
});

test("humans: member people only, de-duplicated and capped", () => {
  const identities = [
    human(1, "Jiho"),
    human(2, "Jiho"),
    agent(3, "Coder"),
    human(4, "Not member"),
    ...Array.from({ length: 12 }, (_, i) => human(10 + i, `H${i}`)),
  ];
  const members = new Set([
    pk(1),
    pk(2),
    pk(3),
    ...identities.slice(4).map((i) => i.pubkey),
  ]);
  const names = routerHumanNames(identities, members);
  assert.equal(names[0], "Jiho");
  assert.ok(!names.includes("Coder"));
  assert.ok(!names.includes("Not member"));
  assert.equal(names.length, MAX_ROUTER_HUMANS);
});
