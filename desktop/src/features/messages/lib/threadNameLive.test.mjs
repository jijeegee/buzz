import assert from "node:assert/strict";
import test from "node:test";
import { QueryClient, QueryObserver } from "@tanstack/react-query";

import {
  applyLiveThreadName,
  mergeThreadName,
  startThreadNameLive,
  threadNameQueryKey,
} from "./threadNameLive.ts";

const name = (id, content, created_at, room = "a", head = "h") => ({
  id,
  kind: 40009,
  pubkey: "b".repeat(64),
  content,
  created_at,
  tags: [
    ["h", room],
    ["e", head],
  ],
  sig: "",
});

const client = () =>
  new QueryClient({ defaultOptions: { queries: { retry: false } } });

test("a live rename updates a row the app holds; newest wins", () => {
  const queryClient = client();
  const key = threadNameQueryKey("a", "h");
  queryClient.setQueryData(key, name("e1", "old", 10));
  applyLiveThreadName(queryClient, name("e2", "new", 20));
  assert.equal(queryClient.getQueryData(key).content, "new");
  applyLiveThreadName(queryClient, name("e0", "older", 5));
  assert.equal(queryClient.getQueryData(key).content, "new");
});

test("a rename for a row nobody shows creates nothing", () => {
  const queryClient = client();
  applyLiveThreadName(queryClient, name("e1", "name", 10, "a", "unseen"));
  assert.equal(
    queryClient.getQueryState(threadNameQueryKey("a", "unseen")),
    undefined,
  );
});

test("a rename during the first read survives the read's older answer", async () => {
  const queryClient = client();
  let answer;
  const read = new Promise((resolve) => {
    answer = resolve;
  });
  const fetching = queryClient.fetchQuery({
    queryKey: threadNameQueryKey("a", "h"),
    queryFn: async () => mergeThreadName(queryClient, "a", "h", await read),
  });
  // The read is in flight: the key exists without data.
  applyLiveThreadName(queryClient, name("live", "renamed", 30));
  answer(name("stale", "before", 10));
  assert.equal((await fetching).content, "renamed");
});

test("a refetch's older answer never replaces a live rename", async () => {
  const queryClient = client();
  const key = threadNameQueryKey("a", "h");
  queryClient.setQueryData(key, name("old", "before", 10));
  let answer;
  const read = new Promise((resolve) => {
    answer = resolve;
  });
  const refetch = queryClient.fetchQuery({
    queryKey: key,
    queryFn: async () => mergeThreadName(queryClient, "a", "h", await read),
  });
  applyLiveThreadName(queryClient, name("live", "renamed", 30));
  answer(name("old", "before", 10));
  assert.equal((await refetch).content, "renamed");
});

test("the live listener applies bundle metadata, and a reconnect re-reads rows on screen only", async () => {
  const queryClient = client();
  let metadata;
  let reconnect;
  const stop = startThreadNameLive(queryClient, {
    listenRoomMetadata: (listener) => {
      metadata = listener;
      return () => {
        metadata = null;
      };
    },
    subscribeToReconnects: (listener) => {
      reconnect = listener;
      return () => {
        reconnect = null;
      };
    },
  });
  const reads = { shown: 0, hidden: 0 };
  const shownKey = threadNameQueryKey("a", "shown");
  const observer = new QueryObserver(queryClient, {
    queryKey: shownKey,
    queryFn: async () => {
      reads.shown += 1;
      return null;
    },
    staleTime: Number.POSITIVE_INFINITY,
  });
  const unsubscribe = observer.subscribe(() => {});
  await queryClient.fetchQuery({
    queryKey: threadNameQueryKey("a", "hidden"),
    queryFn: async () => {
      reads.hidden += 1;
      return null;
    },
  });
  await new Promise((resolve) => setTimeout(resolve, 0));
  metadata(name("e1", "live name", 10, "a", "shown"));
  assert.equal(queryClient.getQueryData(shownKey).content, "live name");

  const before = { ...reads };
  reconnect();
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(reads.shown, before.shown + 1);
  assert.equal(reads.hidden, before.hidden);
  unsubscribe();
  stop();
  assert.equal(metadata, null);
  assert.equal(reconnect, null);
});
