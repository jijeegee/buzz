import assert from "node:assert/strict";
import { afterEach, test } from "node:test";

// Settings › Account › Devices: the owner picks each device's robot, and the
// pick (like a rename) shows up at once on every agent robot for that device.

const OWNER = "a".repeat(64);
const AGENT = "c".repeat(64);
const DEVICE_ID = "3f2504e0-4f89-41d3-9a0c-0305e82c3301";

let devices;
let listDevicesHangs = false;
const tauriCalls = [];
const tauriMock = {
  invoke(command, args) {
    tauriCalls.push({ command, args });
    switch (command) {
      case "rename_device": {
        const device = devices.find((d) => d.id === args.deviceId);
        device.name = args.name;
        return Promise.resolve({ ...device });
      }
      case "list_devices":
        return listDevicesHangs
          ? new Promise(() => {})
          : Promise.resolve(devices.map((d) => ({ ...d })));
      default:
        return Promise.reject(new Error(`unmocked Tauri command: ${command}`));
    }
  },
  transformCallback() {
    return Math.random();
  },
  unregisterCallback() {},
};
globalThis.__TAURI_INTERNALS__ = tauriMock;
globalThis.window.__TAURI_INTERNALS__ = tauriMock;

const { act, cleanup, fireEvent, render, waitFor } = await import(
  "@testing-library/react"
);
const { QueryClient, QueryClientProvider } = await import(
  "@tanstack/react-query"
);
const React = (await import("react")).default;
const { usersBatchEntryKey } = await import("@/features/profile/hooks");
const { MessageAgentOwner } = await import(
  "@/features/messages/ui/MessageAgentOwner.tsx"
);
const { ownerDevicesQueryKey, publishDeviceRobot } = await import(
  "@/shared/api/useOwnerDevices"
);
const { DEVICE_ROBOT_SHAPES, deviceRobotVariantForDevice } = await import(
  "@/shared/lib/deviceRobot"
);
const { TooltipProvider } = await import("@/shared/ui/tooltip");
const { AccountDeviceRow } = await import("./AccountDeviceRow.tsx");

afterEach(() => cleanup());

function setup({ publish = async () => {} } = {}) {
  devices = [
    {
      id: DEVICE_ID,
      name: "Work PC",
      platform: "desktop",
      lastSeenAt: null,
      current: false,
    },
  ];
  tauriCalls.length = 0;
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  queryClient.setQueryData(["identity"], { pubkey: OWNER });
  queryClient.setQueryData(ownerDevicesQueryKey(OWNER), {
    hostDevices: new Map([[AGENT, DEVICE_ID]]),
    robotOverrides: new Map(),
  });
  queryClient.setQueryData(
    ["auth-devices"],
    devices.map((d) => ({ ...d })),
  );
  queryClient.setQueryData(usersBatchEntryKey(AGENT), {
    fetchedAt: Date.now(),
    summary: {
      avatarUrl: null,
      displayName: "Agent",
      isAgent: true,
      nip05Handle: null,
      ownerPubkey: OWNER,
    },
  });
  const signed = [];
  const published = [];
  const errors = [];
  const publishRobot = (client, input) =>
    publishDeviceRobot(client, input, {
      sign: async (draft) => {
        signed.push(draft);
        return {
          id: "e".repeat(64),
          pubkey: OWNER,
          created_at: draft.createdAt,
          kind: draft.kind,
          tags: draft.tags,
          content: draft.content,
          sig: "",
        };
      },
      publish: async (event) => {
        published.push(event);
        await publish(event);
      },
      now: () => 1_700_000_000_000,
    });
  const run = async (_label, action) => {
    try {
      await action();
    } catch (error) {
      errors.push(error);
    }
  };
  const view = render(
    React.createElement(
      QueryClientProvider,
      { client: queryClient },
      React.createElement(
        TooltipProvider,
        null,
        React.createElement(
          "ul",
          null,
          React.createElement(AccountDeviceRow, {
            device: devices[0],
            disabled: false,
            onChanged: async () => {},
            onSignOut: () => {},
            publishRobot,
            run,
          }),
        ),
        React.createElement(MessageAgentOwner, {
          agentPubkey: AGENT,
          ownerLabel: "You",
          ownerPubkey: OWNER,
        }),
      ),
    ),
  );
  return { errors, published, queryClient, signed, view };
}

function robotShapes(container) {
  return [...container.querySelectorAll("[data-testid=device-robot-icon]")].map(
    (robot) => robot.getAttribute("data-robot-shape"),
  );
}

function otherShape() {
  const hashed = deviceRobotVariantForDevice(DEVICE_ID);
  return {
    hashed,
    shape: DEVICE_ROBOT_SHAPES.find((shape) => shape !== hashed.shape),
  };
}

async function pickShape(view, shape) {
  const trigger = view.getByRole("button", {
    name: "Change robot for Work PC",
  });
  await act(async () => {
    fireEvent.click(trigger);
  });
  const option = await view.findByRole("button", {
    name: `${shape.charAt(0).toUpperCase()}${shape.slice(1)} robot`,
  });
  await act(async () => {
    fireEvent.click(option);
  });
}

test("the picker shows every shape and colour as real robots", async () => {
  const { view } = setup();
  await act(async () => {
    fireEvent.click(
      view.getByRole("button", { name: "Change robot for Work PC" }),
    );
  });
  const picker = await view.findByTestId("device-robot-picker");
  const robots = picker.querySelectorAll("[data-testid=device-robot-icon]");
  assert.equal(robots.length, 6 + 8);
  assert.deepEqual(
    [...robots].slice(0, 6).map((r) => r.getAttribute("data-robot-shape")),
    [...DEVICE_ROBOT_SHAPES],
  );
});

test("picking a robot publishes kind 30181 and updates every robot at once", async () => {
  const { hashed, shape } = otherShape();
  let release;
  const { published, signed, view } = setup({
    publish: () =>
      new Promise((resolve) => {
        release = resolve;
      }),
  });
  assert.deepEqual(robotShapes(view.container), [hashed.shape, hashed.shape]);

  await pickShape(view, shape);
  // Before the relay answers, the settings row and the agent's robot in chat
  // already show the new pick.
  assert.equal(published.length, 1);
  // React Query notifies observers on the next tick.
  await waitFor(() =>
    assert.deepEqual(robotShapes(view.container), [shape, shape]),
  );
  const agentRobot = view.container.querySelector(
    "[data-testid=message-agent-owner] [data-testid=device-robot-icon]",
  );
  assert.equal(agentRobot.getAttribute("data-robot-shape"), shape);

  assert.deepEqual(signed, [
    {
      kind: 30180 + 1,
      content: JSON.stringify({ v: 1, shape, color: hashed.colorIndex }),
      createdAt: 1_700_000_000,
      tags: [["d", DEVICE_ID]],
    },
  ]);
  await act(async () => release());
});

test("a failed publish restores the previous robot", async () => {
  const { hashed, shape } = otherShape();
  const { errors, queryClient, view } = setup({
    publish: async () => {
      throw new Error("relay said no");
    },
  });
  await pickShape(view, shape);
  await waitFor(() => assert.equal(errors.length, 1));
  assert.equal(
    queryClient
      .getQueryData(ownerDevicesQueryKey(OWNER))
      .robotOverrides.has(DEVICE_ID),
    false,
  );
  const agentRobot = view.container.querySelector(
    "[data-testid=message-agent-owner] [data-testid=device-robot-icon]",
  );
  assert.equal(agentRobot.getAttribute("data-robot-shape"), hashed.shape);
});

test("renaming a device updates the agent tooltip name immediately", async () => {
  // The refetch never answers, so only the direct cache write can show the
  // new name — a bare invalidation would leave "Work PC" up.
  listDevicesHangs = true;
  const { queryClient, view } = setup();
  const agentRobot = () =>
    view.container.querySelector(
      "[data-testid=message-agent-owner] [data-device-id]",
    );
  assert.equal(agentRobot().getAttribute("aria-label"), "Running on Work PC");

  await act(async () => {
    fireEvent.click(view.getByRole("button", { name: "Rename Work PC" }));
  });
  const input = view.getByRole("textbox", { name: "New name for Work PC" });
  await act(async () => {
    fireEvent.change(input, { target: { value: "Studio PC" } });
  });
  await act(async () => {
    fireEvent.submit(input.closest("form"));
  });
  await waitFor(() =>
    assert.equal(
      agentRobot().getAttribute("aria-label"),
      "Running on Studio PC",
    ),
  );
  assert.ok(
    tauriCalls.some((call) => call.command === "rename_device"),
    "renamed through the device API",
  );
  assert.equal(
    queryClient.getQueryState(["auth-devices"]).isInvalidated ||
      queryClient.getQueryState(["auth-devices"]).fetchStatus === "fetching",
    true,
    "the cached device list is also refetched",
  );
  listDevicesHangs = false;
});
