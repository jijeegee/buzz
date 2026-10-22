import assert from "node:assert/strict";
import test from "node:test";

import { chatGridTiles } from "./chatGridLayout.ts";
import {
  normalizePrivateChatName,
  parsePrivateNamesPayload,
  serializePrivateNames,
  withPrivateName,
} from "./chatPrivateNames.ts";
import {
  formatChatMemberTitle,
  groupChatTitleWithNames,
  invitationChatPeople,
  invitationChatTitle,
  invitationsSentText,
  isChatRoom,
  isGroupChat,
  isListedAsChannel,
  otherChatPeople,
  peopleCountLabel,
  resolveGroupChatTitle,
} from "./groupChat.ts";

const ME = "a".repeat(64);
const key = (char) => char.repeat(64);
const profilesFor = (names) =>
  Object.fromEntries(
    Object.entries(names).map(([pubkey, displayName]) => [
      pubkey,
      { displayName },
    ]),
  );
const chat = (memberPubkeys, name = "") => ({ name, memberPubkeys });

test("chat kinds: group chats and 1:1s are chat rooms, not channels", () => {
  const groupChat = { channelType: "stream", groupChat: true };
  const dm = { channelType: "dm" };
  const stream = { channelType: "stream" };
  assert.equal(isGroupChat(groupChat), true);
  assert.equal(isGroupChat(stream), false);
  assert.equal(isChatRoom(groupChat), true);
  assert.equal(isChatRoom(dm), true);
  assert.equal(isChatRoom(stream), false);
  assert.equal(isListedAsChannel(groupChat), false);
  assert.equal(isListedAsChannel(dm), false);
  assert.equal(isListedAsChannel(stream), true);
});

test("otherChatPeople drops me and duplicates, keeping order", () => {
  assert.deepEqual(
    otherChatPeople([key("c"), ME, key("b"), key("C"), key("c")], ME),
    [key("c"), key("b")],
  );
});

test("title: private name beats shared name beats members", () => {
  const profiles = profilesFor({ [key("b")]: "Ann" });
  const channel = chat([ME, key("b")], "Launch crew");
  assert.equal(
    resolveGroupChatTitle({
      channel,
      currentPubkey: ME,
      privateName: "  Mine ",
      profiles,
    }),
    "Mine",
  );
  assert.equal(
    resolveGroupChatTitle({ channel, currentPubkey: ME, profiles }),
    "Launch crew",
  );
  assert.equal(
    resolveGroupChatTitle({
      channel: chat([ME, key("b")], "  "),
      currentPubkey: ME,
      privateName: " ",
      profiles,
    }),
    "Ann",
  );
});

test("title: first three member names, then +N", () => {
  const names = { b: "Ann", c: "Bob", d: "Cy", e: "Dee", f: "Eve" };
  const title = groupChatTitleWithNames({
    channel: chat([ME, ...Object.keys(names).map(key)]),
    currentPubkey: ME,
    nameOf: (pubkey) => names[pubkey[0]],
  });
  assert.equal(title, "Ann, Bob, Cy +2");
});

test("title: nobody else is an empty chat", () => {
  assert.equal(
    groupChatTitleWithNames({
      channel: chat([ME]),
      currentPubkey: ME,
      nameOf: () => "x",
    }),
    "Empty chat",
  );
  assert.equal(formatChatMemberTitle([]), "Empty chat");
});

test("title: unknown members never show keys", () => {
  const title = formatChatMemberTitle([null, " "]);
  assert.doesNotMatch(title, /[0-9a-f]{8}|npub/);
  assert.equal(title.split(", ").length, 2);
});

test("people count and invitation toast copy", () => {
  assert.equal(peopleCountLabel(1), "1 person");
  assert.equal(peopleCountLabel(4), "4 people");
  assert.equal(invitationsSentText(0, 3), null);
  assert.equal(invitationsSentText(1, 3), "Invitation sent to 1 person");
  assert.equal(invitationsSentText(2, 3), "Invitation sent to 2 people");
  assert.equal(invitationsSentText(3, 3), "Invitations sent");
  assert.equal(invitationsSentText(1, 1), "Invitation sent to 1 person");
});

test("invitation chat: listed members, else the inviter", () => {
  const listed = {
    channel: { name: "", memberCount: 5, memberPubkeys: [key("b"), key("c")] },
    inviter: key("d"),
  };
  assert.deepEqual(invitationChatPeople(listed), [key("b"), key("c")]);
  assert.deepEqual(invitationChatPeople({ channel: {}, inviter: key("d") }), [
    key("d"),
  ]);
  const profiles = profilesFor({ [key("b")]: "Ann", [key("c")]: "Bob" });
  assert.equal(invitationChatTitle(listed, profiles), "Ann, Bob +3");
  assert.equal(
    invitationChatTitle(
      { ...listed, channel: { ...listed.channel, name: "Trip" } },
      profiles,
    ),
    "Trip",
  );
});

test("grid picture: single, diagonal, three and 2x2", () => {
  assert.equal(chatGridTiles(0).length, 1);
  assert.equal(chatGridTiles(1)[0].size, 100);
  const pair = chatGridTiles(2);
  assert.equal(pair.length, 2);
  assert.ok(pair[1].left > pair[0].left && pair[1].top > pair[0].top);
  assert.equal(chatGridTiles(3).length, 3);
  assert.equal(chatGridTiles(4).length, 4);
  assert.equal(chatGridTiles(40).length, 4);
});

test("private names: parse keeps only string names", () => {
  const parsed = parsePrivateNamesPayload({
    one: "  Trip  ",
    two: 3,
    three: "",
    "": "x",
  });
  assert.deepEqual([...parsed], [["one", "Trip"]]);
  assert.equal(parsePrivateNamesPayload([]).size, 0);
  assert.equal(parsePrivateNamesPayload(null).size, 0);
});

test("private names: set, clear, cap and round trip", () => {
  let names = withPrivateName(new Map(), "a", "Trip");
  names = withPrivateName(names, "b", "Work");
  names = withPrivateName(names, "a", "Trip 2");
  assert.deepEqual([...names.keys()], ["b", "a"]);
  names = withPrivateName(names, "b", "  ");
  assert.deepEqual([...names], [["a", "Trip 2"]]);
  assert.deepEqual(
    [...parsePrivateNamesPayload(JSON.parse(serializePrivateNames(names)))],
    [["a", "Trip 2"]],
  );
  assert.equal(
    Array.from(normalizePrivateChatName("x".repeat(200))).length,
    80,
  );
});
