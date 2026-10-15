import assert from "node:assert/strict";
import test from "node:test";

import {
  getMentionOffset,
  getMentionOffsets,
  hasMention,
} from "./hasMention.ts";
import {
  bindSelectedMention,
  extractMentionPubkeys,
  selectedMentionLabel,
  selectedMentionLabels,
} from "./extractMentionPubkeys.ts";

// ── Plain @mention ────────────────────────────────────────────────────

test("matches @Name at start of string", () => {
  assert.equal(hasMention("@Alice hello", "Alice"), true);
});

test("matches @Name after whitespace", () => {
  assert.equal(hasMention("hey @Alice", "Alice"), true);
});

test("matches the first member in a parenthesized team expansion", () => {
  assert.equal(hasMention("Launch Team(@Planner @Builder)", "Planner"), true);
  assert.equal(hasMention("Launch Team(@Planner @Builder)", "Builder"), true);
});

test("matches @Name at end of string", () => {
  assert.equal(hasMention("hello @Alice", "Alice"), true);
});

test("match is case-insensitive", () => {
  assert.equal(hasMention("@alice", "Alice"), true);
  assert.equal(hasMention("@ALICE", "Alice"), true);
});

test("does not match without @ prefix", () => {
  assert.equal(hasMention("Alice hello", "Alice"), false);
});

test("does not match @Name embedded in a word (email-style)", () => {
  assert.equal(hasMention("user@Alice.com", "Alice"), false);
});

// ── Bold-wrapped mentions (**@Name**) ─────────────────────────────────

test("matches **@Name** (bold-wrapped)", () => {
  assert.equal(hasMention("**@Alice**", "Alice"), true);
});

test("matches **@Name** after whitespace", () => {
  assert.equal(hasMention("hey **@Alice**", "Alice"), true);
});

test("matches *@Name* (italic-wrapped)", () => {
  assert.equal(hasMention("*@Alice*", "Alice"), true);
});

test("matches ***@Name*** (bold+italic-wrapped)", () => {
  assert.equal(hasMention("***@Alice***", "Alice"), true);
});

test("matches __@Name__ (underscore bold-wrapped)", () => {
  assert.equal(hasMention("__@Alice__", "Alice"), true);
});

test("matches _@Name_ (underscore italic-wrapped)", () => {
  assert.equal(hasMention("_@Alice_", "Alice"), true);
});

test("matches ||@Name|| (spoiler-wrapped)", () => {
  assert.equal(hasMention("||@Alice||", "Alice"), true);
});

test("matches @Name at the end of spoiler content", () => {
  assert.equal(hasMention("||hi @Alice||", "Alice"), true);
});

// ── Boundary conditions ───────────────────────────────────────────────

test("matches @Name followed by punctuation", () => {
  assert.equal(hasMention("@Alice, hello", "Alice"), true);
  assert.equal(hasMention("@Alice!", "Alice"), true);
  assert.equal(hasMention("@Alice.", "Alice"), true);
  assert.equal(hasMention("@Alice?", "Alice"), true);
});

test("matches multi-word display name", () => {
  assert.equal(hasMention("@John Doe said hi", "John Doe"), true);
});

test("matches multi-word display name bold-wrapped", () => {
  assert.equal(hasMention("**@John Doe**", "John Doe"), true);
});

test("handles regex special characters in name", () => {
  assert.equal(hasMention("@alice (admin)", "alice (admin)"), true);
});

test("does not false-positive on partial name match", () => {
  // "Al" should not match inside "@Alice"
  assert.equal(hasMention("@Alice", "Al"), false);
});

test("returns every mention offset", () => {
  const text = "@Fast Fizz Codex and @Fast Fizz";
  assert.deepEqual(getMentionOffsets(text, "Fast Fizz"), [0, 21]);
});

test("selected longer mention excludes a prefix member at the same offset", () => {
  const pubkeys = extractMentionPubkeys({
    text: "Hive Codex(@Fast Fizz Codex)",
    selectedMentions: new Map([["Fast Fizz Codex", "codex-pubkey"]]),
    memberCandidates: [
      {
        kind: "identity",
        pubkey: "fast-fizz-pubkey",
        displayName: "Fast Fizz",
        isMember: true,
        isAgent: true,
      },
      {
        kind: "identity",
        pubkey: "codex-pubkey",
        displayName: "Fast Fizz Codex",
        isMember: true,
        isAgent: true,
      },
    ],
  });

  assert.deepEqual(pubkeys, ["codex-pubkey"]);
});

test("manual longer mention excludes its prefix member", () => {
  const pubkeys = extractMentionPubkeys({
    text: "@Fast Fizz Codex",
    selectedMentions: new Map(),
    memberCandidates: [
      {
        kind: "identity",
        pubkey: "fast-fizz-pubkey",
        displayName: "Fast Fizz",
        isMember: true,
        isAgent: true,
      },
      {
        kind: "identity",
        pubkey: "codex-pubkey",
        displayName: "Fast Fizz Codex",
        isMember: true,
        isAgent: true,
      },
    ],
  });

  assert.deepEqual(pubkeys, ["codex-pubkey"]);
});

test("manual prefix mentions choose the longest name at each offset", () => {
  const pubkeys = extractMentionPubkeys({
    text: "@Fast Fizz Codex, please pair with @Fast Fizz.",
    selectedMentions: new Map(),
    memberCandidates: [
      {
        kind: "identity",
        pubkey: "fast-fizz-pubkey",
        displayName: "Fast Fizz",
        isMember: true,
        isAgent: true,
      },
      {
        kind: "identity",
        pubkey: "codex-pubkey",
        displayName: "Fast Fizz Codex",
        isMember: true,
        isAgent: true,
      },
    ],
  });

  assert.deepEqual(pubkeys, ["fast-fizz-pubkey", "codex-pubkey"]);
});

test("keeps manually typed prefix member mentions at distinct offsets", () => {
  const pubkeys = extractMentionPubkeys({
    text: "@Fast Fizz Codex, please pair with @Fast Fizz.",
    selectedMentions: new Map([["Fast Fizz Codex", "codex-pubkey"]]),
    memberCandidates: [
      {
        kind: "identity",
        pubkey: "fast-fizz-pubkey",
        displayName: "Fast Fizz",
        isMember: true,
        isAgent: true,
      },
      {
        kind: "identity",
        pubkey: "codex-pubkey",
        displayName: "Fast Fizz Codex",
        isMember: true,
        isAgent: true,
      },
    ],
  });

  assert.deepEqual(pubkeys, ["codex-pubkey", "fast-fizz-pubkey"]);
});

// ── Markdown code ─────────────────────────────────────────────────────

test("ignores mentions in inline code", () => {
  assert.equal(hasMention("run `notify @Alice now`", "Alice"), false);
  assert.equal(hasMention("run ``notify `x` @Alice``", "Alice"), false);
});

test("ignores mentions in fenced code blocks", () => {
  assert.equal(
    hasMention("before\n```ts\nnotify(@Alice)\n```\nafter", "Alice"),
    false,
  );
  assert.equal(hasMention("~~~\r\n@Alice\r\n~~~", "Alice"), false);
});

test("ignores mentions in indented code blocks", () => {
  assert.equal(hasMention("before\n    @Alice\nafter", "Alice"), false);
  assert.equal(hasMention("before\n\t@Alice\nafter", "Alice"), false);
});

test("still matches prose mentions around code", () => {
  assert.equal(hasMention("`@Alice` then @Alice", "Alice"), true);
  assert.equal(hasMention("```\n@Alice\n```\n@Alice", "Alice"), true);
  assert.equal(hasMention("    @Alice\n@Alice", "Alice"), true);
});

test("preserves the original offset after masked code", () => {
  const text = "`@Alice` then @Alice";
  assert.equal(getMentionOffset(text, "Alice"), text.lastIndexOf("@Alice"));
});

test("does not treat escaped or unclosed backticks as code", () => {
  assert.equal(hasMention("\\` @Alice", "Alice"), true);
  assert.equal(hasMention("` @Alice", "Alice"), true);
});

test("requires matching inline-code delimiter lengths", () => {
  assert.equal(hasMention("`` @Alice ` still code ``", "Alice"), false);
  assert.equal(hasMention("`` @Alice `", "Alice"), true);
});

for (const selected of [false, true]) {
  test(`duplicate names ${selected ? "stay bound to selection after rename" : "require explicit selection"}`, () => {
    const opts = {
      text: "@Scout hello",
      selectedMentions: new Map(selected ? [["Scout", "first"]] : []),
      memberCandidates: [
        {
          pubkey: "first",
          displayName: selected ? "Renamed" : "Scout",
          isMember: true,
        },
        { pubkey: "second", displayName: "Scout", isMember: true },
      ],
    };
    if (selected) assert.deepEqual(extractMentionPubkeys(opts), ["first"]);
    else
      assert.throws(
        () => extractMentionPubkeys(opts),
        /ambiguous.*Choose a recipient/,
      );
  });
}

test("a second same-name selection cannot redirect the first mention", () => {
  const selected = new Map([["Scout", "first"]]);
  const repeated = [];
  const secondLabel = selectedMentionLabel("Scout", "second", selected);
  assert.equal(secondLabel, "Scout");
  assert.equal(
    bindSelectedMention(selected, repeated, secondLabel, "second"),
    false,
  );
  assert.equal(selected.get("Scout"), "first");
  assert.deepEqual(
    extractMentionPubkeys({
      text: `@Scout and @${secondLabel}`,
      selectedMentions: selected,
      repeatedMentions: repeated,
      memberCandidates: [],
    }),
    ["first", "second"],
  );
});

test("qualified-looking names cannot redirect an existing selection", () => {
  const selected = new Map([
    ["Scout", "first"],
    ["Scout (second)", "third"],
  ]);
  const repeated = [];
  const label = selectedMentionLabel("Scout", "second", selected);
  assert.equal(label, "Scout");
  bindSelectedMention(selected, repeated, label, "second");
  assert.equal(selected.get("Scout (second)"), "third");
  assert.deepEqual(
    extractMentionPubkeys({
      text: "@Scout and @Scout (second) and @Scout",
      selectedMentions: selected,
      repeatedMentions: repeated,
      memberCandidates: [],
    }),
    ["first", "third", "second"],
  );
});

test("same-name teammates are bound sequentially without replacing either recipient", () => {
  const selections = selectedMentionLabels(
    [
      { displayName: "Scout", pubkey: "first" },
      { displayName: "Scout", pubkey: "second" },
    ],
    new Map(),
  );
  const bindings = new Map();
  const repeated = [];
  for (const { displayName, pubkey } of selections) {
    bindSelectedMention(bindings, repeated, displayName, pubkey);
  }
  const text = selections.map((s) => `@${s.displayName}`).join(" ");
  assert.equal(text, "@Scout @Scout");
  assert.deepEqual(
    extractMentionPubkeys({
      text,
      selectedMentions: bindings,
      repeatedMentions: repeated,
      memberCandidates: [],
    }),
    ["first", "second"],
  );
});

test("releasing a same-name primary promotes the next pick", async () => {
  const { releaseSelectedMention } = await import(
    "./extractMentionPubkeys.ts"
  );
  const selected = new Map();
  const repeated = [];
  for (const pubkey of ["first", "second", "third"]) {
    bindSelectedMention(selected, repeated, "Scout", pubkey);
  }
  releaseSelectedMention(selected, repeated, "first");
  assert.equal(selected.get("Scout"), "second");
  assert.deepEqual(
    extractMentionPubkeys({
      text: "@Scout @Scout",
      selectedMentions: selected,
      repeatedMentions: repeated,
      memberCandidates: [],
    }),
    ["second", "third"],
  );
});

// ── Unnamed ("Unknown user") recipients ───────────────────────────────

const UNNAMED_A = "1".repeat(64);
const UNNAMED_B = "2".repeat(64);
const NAMED = "3".repeat(64);
const KEY_TEXT = /[0-9a-f]{16}|npub1/i;

/** Mirror the composer: pick each identity, insert `@label `, bind it. */
function pickMentions(picks) {
  const selectedMentions = new Map();
  const repeatedMentions = [];
  let text = "";
  for (const { displayName, pubkey } of picks) {
    const label = selectedMentionLabel(displayName, pubkey, selectedMentions);
    bindSelectedMention(selectedMentions, repeatedMentions, label, pubkey);
    text += `@${label} `;
  }
  return { selectedMentions, repeatedMentions, text: text.trim() };
}

test("two different unnamed users keep a keyless label and both recipients", () => {
  const { selectedMentions, repeatedMentions, text } = pickMentions([
    { displayName: "Unknown user", pubkey: UNNAMED_A },
    { displayName: "Unknown user", pubkey: UNNAMED_B },
  ]);
  assert.equal(text, "@Unknown user @Unknown user");
  assert.doesNotMatch(text, KEY_TEXT);
  assert.deepEqual(
    extractMentionPubkeys({
      text: `hey ${text} welcome`,
      selectedMentions,
      repeatedMentions,
      memberCandidates: [],
    }),
    [UNNAMED_A, UNNAMED_B],
  );
});

test("unnamed and named recipients resolve together in one message", () => {
  const { selectedMentions, repeatedMentions, text } = pickMentions([
    { displayName: "Unknown user", pubkey: UNNAMED_A },
    { displayName: "Alice", pubkey: NAMED },
    { displayName: "Unknown user", pubkey: UNNAMED_B },
  ]);
  assert.equal(text, "@Unknown user @Alice @Unknown user");
  assert.deepEqual(
    extractMentionPubkeys({
      text,
      selectedMentions,
      repeatedMentions,
      memberCandidates: [
        { pubkey: NAMED, displayName: "Alice", isMember: true },
        { pubkey: UNNAMED_A, displayName: null, isMember: true },
      ],
    }),
    [UNNAMED_A, NAMED, UNNAMED_B],
  );
});

test("unnamed recipients are bound by selection order to remaining occurrences", () => {
  const { selectedMentions, repeatedMentions } = pickMentions([
    { displayName: "Unknown user", pubkey: UNNAMED_A },
    { displayName: "Unknown user", pubkey: UNNAMED_B },
  ]);
  const extract = (text) =>
    extractMentionPubkeys({
      text,
      selectedMentions,
      repeatedMentions,
      memberCandidates: [],
    });
  assert.deepEqual(extract("only @Unknown user now"), [UNNAMED_A]);
  assert.deepEqual(extract("no mentions left"), []);
  assert.deepEqual(extract("`@Unknown user` @Unknown user"), [UNNAMED_A]);
});

test("re-picking the same unnamed user does not queue a duplicate", () => {
  const { selectedMentions, repeatedMentions, text } = pickMentions([
    { displayName: "Unknown user", pubkey: UNNAMED_A },
    { displayName: "Unknown user", pubkey: UNNAMED_B },
    { displayName: "Unknown user", pubkey: UNNAMED_B },
    { displayName: "Unknown user", pubkey: UNNAMED_A },
  ]);
  assert.equal(selectedMentions.size, 1);
  assert.deepEqual(
    repeatedMentions.map((repeated) => repeated.pubkey),
    [UNNAMED_B],
  );
  assert.deepEqual(
    extractMentionPubkeys({
      text,
      selectedMentions,
      repeatedMentions,
      memberCandidates: [],
    }),
    [UNNAMED_A, UNNAMED_B],
  );
});

test("unnamed team members are labelled without key material", () => {
  const labelled = selectedMentionLabels(
    [
      { displayName: "Unknown user", pubkey: UNNAMED_A },
      { displayName: "Unknown user", pubkey: UNNAMED_B },
    ],
    new Map([["Unknown user", NAMED]]),
  );
  assert.deepEqual(
    labelled.map((member) => member.displayName),
    ["Unknown user", "Unknown user"],
  );
});

test("named same-label selections use the same keyless ordered binding", () => {
  const { selectedMentions, repeatedMentions, text } = pickMentions([
    { displayName: "Scout", pubkey: "first" },
    { displayName: "Alice", pubkey: NAMED },
    { displayName: "Scout", pubkey: "second" },
  ]);
  assert.equal(text, "@Scout @Alice @Scout");
  assert.equal(selectedMentions.size, 2);
  assert.deepEqual(
    repeatedMentions.map((repeated) => repeated.pubkey),
    ["second"],
  );
  assert.deepEqual(
    extractMentionPubkeys({
      text,
      selectedMentions,
      repeatedMentions,
      memberCandidates: [],
    }),
    ["first", NAMED, "second"],
  );
});

test("draft refs include repeated picks only while their occurrences remain", async () => {
  const { snapshotDraftMentionRefs } = await import("./draftMentionRefs.ts");
  const { selectedMentions, repeatedMentions } = pickMentions([
    { displayName: "Scout", pubkey: UNNAMED_A },
    { displayName: "Scout", pubkey: UNNAMED_B },
  ]);
  const refsFor = (text) =>
    snapshotDraftMentionRefs(
      text,
      selectedMentions,
      [],
      [],
      [],
      [],
      [],
      repeatedMentions,
    ).map((ref) => ref.pubkey);
  assert.deepEqual(refsFor("@Scout @Scout"), [UNNAMED_A, UNNAMED_B]);
  assert.deepEqual(refsFor("@Scout"), [UNNAMED_A]);
  assert.deepEqual(refsFor("none"), []);
});

test("draft refs round-trip repeated unnamed recipients", async () => {
  const { replaceWithDraftMentionRefs, snapshotDraftMentionRefs } =
    await import("./draftMentionRefs.ts");
  const { selectedMentions, repeatedMentions, text } = pickMentions([
    { displayName: "Unknown user", pubkey: UNNAMED_A },
    { displayName: "Unknown user", pubkey: UNNAMED_B },
  ]);
  const refs = snapshotDraftMentionRefs(
    text,
    selectedMentions,
    [],
    [],
    [],
    [],
    [],
    repeatedMentions,
  );
  assert.deepEqual(
    refs.map((ref) => ref.pubkey),
    [UNNAMED_A, UNNAMED_B],
  );
  const restored = new Map();
  const restoredRepeated = [];
  replaceWithDraftMentionRefs(refs, restored, new Map(), restoredRepeated);
  assert.deepEqual(
    extractMentionPubkeys({
      text,
      selectedMentions: restored,
      repeatedMentions: restoredRepeated,
      memberCandidates: [],
    }),
    [UNNAMED_A, UNNAMED_B],
  );
});
