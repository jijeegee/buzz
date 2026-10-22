import assert from "node:assert/strict";
import test from "node:test";

import { newChatPeopleFromDm } from "./AddPeopleToDmDialog.tsx";
import { joinPeopleNames, startChatPromptText } from "./StartChatConfirm.tsx";
import { peopleOutsideDm } from "./usePrepareDmSendChannel.ts";

const ME = "a".repeat(64);
const key = (char) => char.repeat(64);

test("1:1 Add people: partner first, then picks, without me or repeats", () => {
  assert.deepEqual(
    newChatPeopleFromDm([ME, key("b")], [key("c"), key("b"), ME], ME),
    [key("b"), key("c")],
  );
});

test("mentions outside the 1:1 are the ones that start a new chat", () => {
  assert.deepEqual(
    peopleOutsideDm([ME, key("b")], [key("b"), key("c"), ME, key("c")], ME),
    [key("c")],
  );
  assert.deepEqual(peopleOutsideDm([ME, key("b")], [key("b")], ME), []);
});

test("start-chat prompt copy", () => {
  assert.equal(joinPeopleNames(["Ann"]), "Ann");
  assert.equal(joinPeopleNames(["Ann", "Bob"]), "Ann and Bob");
  assert.equal(joinPeopleNames(["Ann", "Bob", "Cy"]), "Ann, Bob and Cy");
  assert.equal(joinPeopleNames(["A", "B", "C", "D", "E"]), "A, B, C +2");

  const one = startChatPromptText(["Ann"]);
  assert.equal(one.title, "Start a new chat with Ann?");
  assert.match(one.description, /^Ann isn't in this chat\./);
  const two = startChatPromptText(["Ann", "Bob"]);
  assert.match(two.description, /^Ann and Bob aren't in this chat\./);
  assert.doesNotMatch(`${one.title} ${two.description}`, /group/i);
});
