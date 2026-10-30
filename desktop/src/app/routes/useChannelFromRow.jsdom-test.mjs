/**
 * A room the channel list does not hold opens from its Chats row instead of
 * a loading screen, until the relay says the room does not exist.
 *
 * The row's channel only stands in for display: no members, flagged so the
 * pane takes no sends, and a DM titled by stored names or a short id.
 *
 * Falsifiability: returning null from useChannelFromRow (the old behaviour)
 * fails the first test; dropping the not-found check fails the second.
 */
import assert from "node:assert/strict";
import { afterEach, test } from "node:test";

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import React, { act } from "react";
import { createRoot } from "react-dom/client";

import { channelsQueryKey } from "../../features/channels/hooks.ts";
import { channelReferenceQueryKey } from "../../features/channels/openChannelDirectory.ts";
import {
  channelFromChatRow,
  isChatRowStandIn,
  publishShownChatRows,
} from "../../features/chats/chatRowChannel.ts";
import { setStoreScope } from "../../features/messages/lib/paintCoverage.ts";
import {
  resetUserLabelsForTests,
  writeCachedUserLabels,
} from "../../features/profile/lib/userLabelStorage.ts";
import { truncateNpub } from "../../shared/lib/pubkey.ts";
import { useChannelFromRow } from "./useChannelFromRow.ts";

const ME = "aa".repeat(32);
const PEER = "cc".repeat(32);

function row(extra = {}) {
  return {
    type: "room",
    key: "dm-1",
    at: 100,
    at_us: 100_000_000,
    channel_id: "dm-1",
    channel_type: "dm",
    channel_name: "",
    archived: false,
    dm_members: [PEER],
    root_id: null,
    thread_name: null,
    follow_state: null,
    closed: false,
    last_seq: null,
    read_seq: null,
    unread: 0,
    unread_overflow: false,
    unread_mentions: 0,
    unread_mentions_overflow: false,
    unread_mark: false,
    latest: null,
    root: null,
    ...extra,
  };
}

let cleanup = null;
afterEach(() => {
  cleanup?.();
  cleanup = null;
  resetUserLabelsForTests();
});

function mount(client, props) {
  const seen = [];
  function Probe(probeProps) {
    seen.push(useChannelFromRow(probeProps));
    return null;
  }
  const container = document.createElement("div");
  const root = createRoot(container);
  const render = (next) =>
    act(() => {
      root.render(
        React.createElement(
          QueryClientProvider,
          { client },
          React.createElement(Probe, next),
        ),
      );
    });
  cleanup = () => act(() => root.unmount());
  return { render, seen, initial: render(props) };
}

test("a room missing from the list opens from its Chats row at once", async () => {
  const client = new QueryClient();
  setStoreScope(client, { pubkey: ME, relayUrl: "wss://relay.example" });
  publishShownChatRows(client, [row()]);
  const { initial, seen } = mount(client, {
    channelId: "dm-1",
    enabled: true,
    listLoaded: false,
  });
  await initial;
  const channel = seen.at(-1);
  assert.ok(channel, "opened from the row, no loading screen");
  assert.equal(channel.id, "dm-1");
  assert.equal(channel.channelType, "dm");
  assert.equal(channel.isMember, true);
  assert.ok(isChatRowStandIn(channel), "the pane takes no sends from it");
  assert.deepEqual(channel.participantPubkeys, [], "no members to route to");
  assert.deepEqual(channel.memberPubkeys, []);
  assert.equal(channel.name, truncateNpub(PEER), "a short id, not blank");
});

test("a DM from a row is titled by stored profile names", async () => {
  const client = new QueryClient();
  const scope = { pubkey: ME, relayUrl: "wss://relay.example" };
  setStoreScope(client, scope);
  writeCachedUserLabels(scope, {
    [PEER]: {
      displayName: "Carol",
      name: null,
      avatarUrl: null,
      nip05Handle: null,
      ownerPubkey: null,
    },
  });
  publishShownChatRows(client, [row()]);
  const { initial, seen } = mount(client, {
    channelId: "dm-1",
    enabled: true,
    listLoaded: false,
  });
  await initial;
  assert.equal(seen.at(-1)?.name, "Carol");
});

test("a room the relay says cannot be opened is not found too", async () => {
  const client = new QueryClient();
  publishShownChatRows(client, [row()]);
  const shown = channelFromChatRow(row(), ME);
  client.setQueryData(channelReferenceQueryKey("dm-1"), {
    ...shown,
    isMember: false,
    visibility: "private",
  });
  const { initial, seen } = mount(client, {
    channelId: "dm-1",
    enabled: true,
    listLoaded: true,
  });
  await initial;
  assert.equal(seen.at(-1), null);
});

test("the relay's not-found answer keeps the existing handling", async () => {
  const client = new QueryClient();
  publishShownChatRows(client, [row()]);
  client.setQueryData(channelReferenceQueryKey("dm-1"), null);
  const { initial, seen } = mount(client, {
    channelId: "dm-1",
    enabled: true,
    listLoaded: true,
  });
  await initial;
  assert.equal(seen.at(-1), null);
});

test("disabled (the list has the room) or no row: nothing", async () => {
  const client = new QueryClient();
  publishShownChatRows(client, [row()]);
  const { initial, seen, render } = mount(client, {
    channelId: "dm-1",
    enabled: false,
    listLoaded: true,
  });
  await initial;
  assert.equal(seen.at(-1), null);
  await render({ channelId: "other", enabled: true, listLoaded: true });
  assert.equal(seen.at(-1), null);
});

test("a loaded list lacking the room is asked again once", async () => {
  const client = new QueryClient();
  publishShownChatRows(client, [row()]);
  const invalidated = [];
  const original = client.invalidateQueries.bind(client);
  client.invalidateQueries = (filters) => {
    invalidated.push(filters);
    return original(filters);
  };
  const { initial, render } = mount(client, {
    channelId: "dm-1",
    enabled: true,
    listLoaded: true,
  });
  await initial;
  await render({ channelId: "dm-1", enabled: true, listLoaded: true });
  assert.deepEqual(invalidated, [{ queryKey: channelsQueryKey, exact: true }]);
});

test("rows arriving later open the room without remounting", async () => {
  const client = new QueryClient();
  const { initial, seen } = mount(client, {
    channelId: "dm-1",
    enabled: true,
    listLoaded: false,
  });
  await initial;
  assert.equal(seen.at(-1), null);
  await act(async () => publishShownChatRows(client, [row()]));
  assert.equal(seen.at(-1)?.id, "dm-1");
});

test("a group row keeps its name and kind, names no members", () => {
  const group = channelFromChatRow(
    row({
      channel_type: "stream",
      channel_name: "Team",
      group_chat: true,
      member_count: 5,
      dm_members: [PEER, ME],
    }),
    ME,
  );
  assert.equal(group.channelType, "stream");
  assert.equal(group.name, "Team");
  assert.equal(group.groupChat, true);
  assert.equal(group.memberCount, 5);
  assert.deepEqual(group.memberPubkeys, []);
  const unnamed = channelFromChatRow(
    row({ channel_type: "stream", group_chat: true, dm_members: [PEER, ME] }),
    ME,
    () => "Carol",
  );
  assert.equal(unnamed.name, "Carol");
});
