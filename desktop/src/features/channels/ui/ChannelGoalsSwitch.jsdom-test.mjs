import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

// The per-room goals switch in channel settings. Binds the production seam:
// the real `useChannelGoalsSwitch` (the hook ChannelManagementSheet uses)
// behind the real `ChannelGoalsSwitchRow`, down to the `update_channel`
// Tauri command and its payload.

const CHANNEL_ID = "11111111-1111-4111-8111-111111111111";
const calls = [];
let failNextUpdate = false;

function rawDetail(input) {
  return {
    id: input.channelId,
    name: "general",
    channel_type: "stream",
    visibility: "open",
    description: "",
    topic: null,
    purpose: null,
    member_count: 1,
    member_pubkeys: [],
    last_message_at: null,
    archived_at: null,
    participants: [],
    participant_pubkeys: [],
    ttl_seconds: null,
    ttl_deadline: null,
    goals_enabled: input.goalsEnabled ?? true,
    created_by: "ab".repeat(32),
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    topic_set_by: null,
    topic_set_at: null,
    purpose_set_by: null,
    purpose_set_at: null,
    topic_required: false,
    max_members: null,
    nip29_group_id: null,
  };
}

const tauriMock = {
  invoke(command, args) {
    calls.push({ command, args });
    if (command === "update_channel") {
      if (failNextUpdate) {
        failNextUpdate = false;
        return Promise.reject(new Error("relay rejected the edit"));
      }
      return Promise.resolve(rawDetail(args.input));
    }
    return Promise.reject(new Error(`unmocked Tauri command: ${command}`));
  },
  transformCallback() {
    return Math.random();
  },
};
globalThis.__TAURI_INTERNALS__ = tauriMock;
globalThis.window.__TAURI_INTERNALS__ = tauriMock;

const { fireEvent } = await import("@testing-library/react");
const React = (await import("react")).default;
const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { QueryClient, QueryClientProvider } = await import(
  "@tanstack/react-query"
);
const { ChannelGoalsSwitchRow } = await import("./ChannelGoalsSwitchRow.tsx");
const { useChannelGoalsSwitch } = await import("./useChannelGoalsSwitch.ts");

function Harness({ channel, canManageChannel }) {
  const goals = useChannelGoalsSwitch(channel, canManageChannel);
  return React.createElement(ChannelGoalsSwitchRow, {
    canToggle: goals.canToggle,
    checked: goals.enabled,
    error: goals.error,
    isSaving: goals.isSaving,
    onCheckedChange: goals.setEnabled,
  });
}

async function settle(predicate) {
  for (let i = 0; i < 40 && !predicate(); i += 1) {
    await act(() => new Promise((resolve) => setTimeout(resolve, 5)));
  }
}

async function mount({ channelType = "stream", goalsEnabled, canManage }) {
  calls.length = 0;
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  const channel = { channelType, goalsEnabled, id: CHANNEL_ID };
  await act(async () => {
    root.render(
      React.createElement(
        QueryClientProvider,
        { client: queryClient },
        React.createElement(Harness, {
          canManageChannel: canManage,
          channel,
        }),
      ),
    );
  });
  const control = () =>
    container.querySelector("[data-testid='channel-management-goals-switch']");
  return {
    container,
    control,
    unmount: async () => {
      await act(async () => root.unmount());
      container.remove();
    },
  };
}

function updateCalls() {
  return calls.filter((call) => call.command === "update_channel");
}

test("the switch is a labelled control with its helper text as description", async () => {
  const { container, control, unmount } = await mount({
    canManage: true,
    goalsEnabled: true,
  });
  const el = control();
  assert.equal(el.getAttribute("role"), "switch");
  assert.equal(el.getAttribute("aria-checked"), "true");
  const label = container.querySelector(`label[for='${el.id}']`);
  assert.equal(label?.textContent, "Goals");
  const hint = document.getElementById(el.getAttribute("aria-describedby"));
  assert.match(hint.textContent, /Track goals for this conversation/);
  await unmount();
});

test("an admin turning the switch off sends a goals-only update", async () => {
  const { control, unmount } = await mount({
    canManage: true,
    goalsEnabled: true,
  });
  await act(async () => {
    fireEvent.click(control());
  });
  await settle(() => updateCalls().length > 0);
  assert.deepEqual(
    updateCalls().map((call) => call.args),
    [{ input: { channelId: CHANNEL_ID, goalsEnabled: false } }],
  );
  await unmount();
});

test("turning the switch on sends goalsEnabled true", async () => {
  const { control, unmount } = await mount({
    canManage: true,
    goalsEnabled: false,
  });
  assert.equal(control().getAttribute("aria-checked"), "false");
  await act(async () => {
    fireEvent.click(control());
  });
  await settle(() => updateCalls().length > 0);
  assert.deepEqual(updateCalls()[0].args, {
    input: { channelId: CHANNEL_ID, goalsEnabled: true },
  });
  await unmount();
});

test("non-admins in a channel see the state read-only", async () => {
  const { container, control, unmount } = await mount({
    canManage: false,
    goalsEnabled: true,
  });
  assert.equal(control().disabled, true);
  assert.equal(control().getAttribute("aria-checked"), "true");
  assert.match(container.textContent, /Only owners and admins can change this/);
  await act(async () => {
    fireEvent.click(control());
  });
  assert.equal(updateCalls().length, 0);
  await unmount();
});

test("any DM participant may flip the switch", async () => {
  const { control, unmount } = await mount({
    canManage: false,
    channelType: "dm",
    goalsEnabled: true,
  });
  assert.equal(control().disabled, false);
  await act(async () => {
    fireEvent.click(control());
  });
  await settle(() => updateCalls().length > 0);
  assert.deepEqual(updateCalls()[0].args, {
    input: { channelId: CHANNEL_ID, goalsEnabled: false },
  });
  await unmount();
});

test("a failed save surfaces the error on the row", async () => {
  failNextUpdate = true;
  const { container, control, unmount } = await mount({
    canManage: true,
    goalsEnabled: true,
  });
  await act(async () => {
    fireEvent.click(control());
  });
  await settle(() => container.querySelector("[role='alert']") !== null);
  assert.match(
    container.querySelector("[role='alert']").textContent,
    /relay rejected the edit/,
  );
  // The saved state (still on) is shown again once the edit failed.
  assert.equal(control().getAttribute("aria-checked"), "true");
  await unmount();
});

test("ChannelManagementSheet wires the switch row and goal views to the hook", () => {
  const source = readFileSync(
    new URL("./ChannelManagementSheet.tsx", import.meta.url),
    "utf8",
  ).replace(/\s+/g, " ");
  assert.match(
    source,
    /useChannelGoalsSwitch\(detail, canManageChannel\)/,
    "the sheet must derive the switch from the channel detail",
  );
  assert.match(source, /const goalsEnabled = goalsSwitch\.enabled;/);
  assert.match(source, /onGoalsEnabledChange=\{goalsSwitch\.setEnabled\}/);
  assert.doesNotMatch(source, /useFeatureEnabled\("goalTree"\)/);
});
