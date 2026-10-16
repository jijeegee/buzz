import assert from "node:assert/strict";
import { afterEach, test } from "node:test";
import React from "react";
import { act, cleanup, render, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { useAutoAssignDeviceRobots } from "./useAutoAssignDeviceRobots.ts";
import { ownerDevicesQueryKey, publishDeviceRobot } from "./useOwnerDevices.ts";

afterEach(cleanup);
const OWNER = "a".repeat(64);
const OTHER = "b".repeat(64);
const DEVICE = "3f2504e0-4f89-41d3-9a0c-0305e82c3301";
const emptyRecords = () => ({
  hostDevices: new Map(),
  robotOverrides: new Map(),
});

test("automatic robot assignment cannot loop after rollback or refetch; owners stay separate", async () => {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, staleTime: Infinity } },
  });
  client.setQueryData(["auth-devices"], [{ id: DEVICE, current: true }]);
  for (const owner of [OWNER, OTHER])
    client.setQueryData(ownerDevicesQueryKey(owner), emptyRecords());
  let attempts = 0;
  const errors = [];
  const originalError = console.error;
  console.error = (...args) => errors.push(args);
  const publish = (queryClient, input) =>
    publishDeviceRobot(queryClient, input, {
      now: () => 1700000000000,
      sign: async (draft) => ({
        id: "e".repeat(64),
        pubkey: input.ownerPubkey,
        kind: draft.kind,
        tags: draft.tags,
        content: draft.content,
        created_at: draft.createdAt,
        sig: "",
      }),
      publish: async () => {
        attempts++;
        throw new Error("invalid: unsupported content version");
      },
    });
  function Harness({ owner }) {
    useAutoAssignDeviceRobots(owner, publish);
    return null;
  }
  const tree = (owner) =>
    React.createElement(
      QueryClientProvider,
      { client },
      React.createElement(Harness, { owner }),
    );
  try {
    const view = render(tree(OWNER));
    await waitFor(() => assert.equal(errors.length, 1));
    assert.equal(
      client.getQueryData(ownerDevicesQueryKey(OWNER)).robotOverrides.size,
      0,
      "production publisher rolled back",
    );
    for (let i = 0; i < 8; i++) {
      await act(async () => {
        client.setQueryData(ownerDevicesQueryKey(OWNER), emptyRecords());
        view.rerender(tree(OWNER));
        await new Promise((resolve) => setTimeout(resolve, 5));
      });
    }
    assert.equal(
      attempts,
      1,
      "query changes must not restart a failed automatic write",
    );
    view.rerender(tree(OTHER));
    await waitFor(() => assert.equal(attempts, 2));
    view.rerender(tree(OWNER));
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 10));
    });
    assert.equal(
      attempts,
      2,
      "switching back must not retry the already attempted identity",
    );
  } finally {
    console.error = originalError;
    client.clear();
  }
});
