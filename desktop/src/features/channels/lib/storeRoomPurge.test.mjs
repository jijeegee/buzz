import assert from "node:assert/strict";
import test from "node:test";

import { QueryClient } from "@tanstack/react-query";

import { channelsQueryKey, refreshChannelsQuery } from "../hooks.ts";
import { setStoreRoomPurgeDeps } from "./storeRoomPurge.ts";

function channel(
  id,
  { channelType = "stream", isMember = true, archivedAt = null } = {},
) {
  return {
    id,
    name: id,
    channelType,
    visibility: channelType === "dm" ? "private" : "open",
    description: "",
    topic: null,
    purpose: null,
    memberCount: 0,
    memberPubkeys: [],
    lastMessageAt: null,
    archivedAt,
    participants: [],
    participantPubkeys: [],
    isMember,
    ttlSeconds: null,
    ttlDeadline: null,
  };
}

function harness() {
  const calls = [];
  const restore = setStoreRoomPurgeDeps({
    forgetExcept: async (roomIds, scope) => {
      calls.push({ roomIds: roomIds.slice().sort(), scope });
    },
  });
  return { calls, restore };
}

function refresh(
  fetchChannels,
  { queryClient = new QueryClient(), initialSnapshotPair = null } = {},
) {
  return refreshChannelsQuery({
    queryClient,
    initialSnapshotPair,
    relayUrl: null,
    ownerPubkey: null,
    fetchChannels,
  });
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

/** A get_channels answer; `memberIds` defaults to the joined channels. */
function payload(
  channels,
  { memberIds, storeScope = "stem:1:7", hash = "h" } = {},
) {
  return {
    hash,
    channels,
    lastMessages: {},
    memberIds:
      memberIds ?? (channels ?? []).filter((c) => c.isMember).map((c) => c.id),
    storeScope,
  };
}

const answer = (channels, options) => async () => payload(channels, options);

test("a fetched list keeps its membership and joined rooms, with the fetch's capture", async () => {
  const { calls, restore } = harness();
  try {
    await refresh(
      answer(
        [
          channel("joined"),
          channel("archived", { archivedAt: "2026-01-01T00:00:00.000Z" }),
          channel("dm", { channelType: "dm", isMember: false }),
          channel("left-open", { isMember: false }),
        ],
        { memberIds: ["joined", "archived"] },
      ),
    );
    await settle();
    assert.deepEqual(calls, [
      { roomIds: ["archived", "dm", "joined"], scope: "stem:1:7" },
    ]);
  } finally {
    restore();
  }
});

test("a hidden DM, absent from the shown list, is kept by its membership", async () => {
  const { calls, restore } = harness();
  try {
    await refresh(
      answer([channel("joined")], { memberIds: ["joined", "hidden-dm"] }),
    );
    await settle();
    assert.deepEqual(calls[0].roomIds, ["hidden-dm", "joined"]);
  } finally {
    restore();
  }
});

test("a room joined while the fetch was in flight is kept", async () => {
  const { calls, restore } = harness();
  try {
    const queryClient = new QueryClient();
    queryClient.setQueryData(channelsQueryKey, [channel("joined")]);
    await refresh(
      async () => {
        // The join lands in the displayed cache before the list returns.
        queryClient.setQueryData(channelsQueryKey, [
          channel("joined"),
          channel("just-joined"),
        ]);
        return payload([channel("joined")]);
      },
      { queryClient },
    );
    await settle();
    assert.deepEqual(calls[0].roomIds, ["joined", "just-joined"]);
  } finally {
    restore();
  }
});

test("a not-modified answer applies its membership and capture too", async () => {
  const { calls, restore } = harness();
  try {
    await refresh(
      answer(null, { memberIds: ["joined", "hidden-dm"], hash: "h" }),
      {
        initialSnapshotPair: { channels: [channel("joined")], hash: "h" },
      },
    );
    await settle();
    assert.deepEqual(calls, [
      { roomIds: ["hidden-dm", "joined"], scope: "stem:1:7" },
    ]);
  } finally {
    restore();
  }
});

test("a failed fetch, an empty membership or no store capture forgets nothing", async () => {
  const { calls, restore } = harness();
  try {
    await assert.rejects(
      refresh(async () => {
        throw new Error("offline");
      }),
    );
    await refresh(answer([channel("left-open", { isMember: false })]));
    await refresh(answer([channel("joined")], { storeScope: null }));
    // No membership at all: a bad answer, even with a room shown joined.
    await refresh(answer([channel("shown")], { memberIds: [] }));
    await settle();
    assert.deepEqual(calls, []);
  } finally {
    restore();
  }
});

test("a list from an older fetch that settles last is not acted on", async () => {
  const { calls, restore } = harness();
  try {
    let releaseOld;
    const old = refresh(
      () =>
        new Promise((resolve) => {
          releaseOld = () => resolve(payload([channel("old")]));
        }),
    );
    await refresh(answer([channel("new")]));
    await settle();
    releaseOld();
    await old;
    await settle();
    assert.deepEqual(calls, [{ roomIds: ["new"], scope: "stem:1:7" }]);
  } finally {
    restore();
  }
});
