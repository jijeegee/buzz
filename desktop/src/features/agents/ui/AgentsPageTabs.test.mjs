/**
 * The Agents page's two tabs. "My agents" lists real agents only: every
 * agent made from a preset gets its own card, and a preset with no agent has
 * none. "Presets" lists the templates with no status: the card opens the
 * preset (a persona target, never an agent pubkey) and "Create agent" asks
 * for a new agent from it.
 */

import assert from "node:assert/strict";
import { after, afterEach, before, test } from "node:test";

import { JSDOM } from "jsdom";

const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://localhost",
});

const clients = [];

let act;
let cleanup;
let fireEvent;
let render;
let screen;
let createElement;
let QueryClient;
let QueryClientProvider;
let MyAgentsSection;
let PresetsSection;

const ipcHandlers = new Map();

const SELF_PK = "c".repeat(64);
const FIRST_PK = "a".repeat(64);
const SECOND_PK = "b".repeat(64);

function agent(overrides = {}) {
  return {
    pubkey: FIRST_PK,
    name: "Fizz",
    personaId: "persona-1",
    status: "stopped",
    model: null,
    modelSource: "global",
    lastError: null,
    lastErrorCode: null,
    needsRestart: false,
    personaOrphaned: false,
    startOnAppLaunch: true,
    backend: { type: "local" },
    ...overrides,
  };
}

function persona(overrides = {}) {
  return {
    id: "persona-1",
    displayName: "Fizz",
    avatarUrl: null,
    model: null,
    isBuiltIn: false,
    sourceTeam: null,
    ...overrides,
  };
}

const noop = () => {};

function myAgentsProps(overrides = {}) {
  return {
    agents: [],
    agentsError: null,
    defaultModel: "gpt-x",
    getAvailability: () => undefined,
    isActionPending: false,
    isAgentsLoading: false,
    isStartOnLaunchPending: false,
    personas: [],
    restartingAgentPubkey: null,
    startingAgentPubkey: null,
    onAddToChannel: noop,
    onEditPersona: noop,
    onOpenAgentProfile: noop,
    onRestartAgent: noop,
    onStartAgent: noop,
    onStopAgent: noop,
    onToggleStartOnLaunch: noop,
    ...overrides,
  };
}

function presetsProps(overrides = {}) {
  return {
    agents: [],
    isActionPending: false,
    isLoading: false,
    personas: [],
    personasError: null,
    startingPersonaIds: new Set(),
    teams: [],
    teamsError: null,
    isTeamsLoading: false,
    isTeamsPending: false,
    onCreateAgent: noop,
    onDeactivatePersona: noop,
    onDeletePersona: noop,
    onDuplicatePersona: noop,
    onEditPersona: noop,
    onImportPreset: noop,
    onOpenPersonaProfile: noop,
    onSharePersona: noop,
    onAddTeamToChannel: noop,
    onCreateTeam: noop,
    onDeleteTeam: noop,
    onDuplicateTeam: noop,
    onEditTeam: noop,
    onImportTeam: noop,
    onShareTeam: noop,
    ...overrides,
  };
}

async function renderWithClient(element) {
  const client = new QueryClient({
    defaultOptions: {
      queries: { retry: false, gcTime: 0 },
      mutations: { gcTime: 0 },
    },
  });
  clients.push(client);
  await act(async () => {
    render(createElement(QueryClientProvider, { client }, element));
  });
}

before(async () => {
  Object.assign(globalThis, {
    document: dom.window.document,
    HTMLElement: dom.window.HTMLElement,
    window: dom.window,
    IS_REACT_ACT_ENVIRONMENT: true,
  });
  Object.defineProperty(globalThis, "navigator", {
    configurable: true,
    value: dom.window.navigator,
    writable: true,
  });
  dom.window.matchMedia = () => ({
    matches: true,
    addEventListener() {},
    removeEventListener() {},
  });
  dom.window.__TAURI_INTERNALS__ = {
    invoke: (cmd, args) => {
      const handler = ipcHandlers.get(cmd);
      if (handler) return handler(args);
      return Promise.reject(new Error(`unmocked Tauri command: ${cmd}`));
    },
    transformCallback: () => Math.random(),
  };

  ({ act, cleanup, fireEvent, render, screen } = await import(
    "@testing-library/react"
  ));
  ({ createElement } = await import("react"));
  ({ QueryClient, QueryClientProvider } = await import(
    "@tanstack/react-query"
  ));
  ({ MyAgentsSection } = await import("./MyAgentsSection.tsx"));
  ({ PresetsSection } = await import("./PresetsSection.tsx"));
});

afterEach(() => {
  cleanup?.();
  for (const client of clients.splice(0)) {
    client.cancelQueries();
    client.clear();
  }
  ipcHandlers.clear();
});

after(() => dom.window.close());

function installIpc() {
  ipcHandlers.set("get_identity", () =>
    Promise.resolve({ pubkey: SELF_PK, display_name: "Me" }),
  );
  ipcHandlers.set("list_archived_identities", () => Promise.resolve([]));
  ipcHandlers.set("get_user_profile", ({ pubkey } = {}) =>
    Promise.resolve({
      pubkey,
      display_name: null,
      avatar_url: null,
      about: null,
      nip05_handle: null,
      owner_pubkey: null,
    }),
  );
}

test("My agents shows every real agent and no preset without one", async () => {
  installIpc();
  await renderWithClient(
    createElement(
      MyAgentsSection,
      myAgentsProps({
        agents: [
          agent({ pubkey: SECOND_PK, name: "Fizz 2", status: "running" }),
          agent(),
        ],
        personas: [
          persona(),
          persona({ id: "persona-2", displayName: "Pollen" }),
        ],
      }),
    ),
  );

  assert.ok(screen.getByRole("button", { name: "Fizz agent profile" }));
  assert.ok(screen.getByRole("button", { name: "Fizz 2 agent profile" }));
  assert.equal(screen.queryByText("Pollen"), null);
  assert.equal(
    screen.getByTestId(`managed-agent-${SECOND_PK}-status`).textContent,
    "Running",
  );
  assert.ok(screen.getByTestId(`agent-tile-start-${FIRST_PK}`));
  assert.ok(screen.getByTestId(`agent-tile-stop-${SECOND_PK}`));
});

test("an agent card opens that agent and its buttons act on it", async () => {
  installIpc();
  const opened = [];
  const stopped = [];
  const added = [];
  await renderWithClient(
    createElement(
      MyAgentsSection,
      myAgentsProps({
        agents: [agent({ status: "running" })],
        personas: [persona()],
        onAddToChannel: (target) => added.push(target.pubkey),
        onOpenAgentProfile: (pubkey) => opened.push(pubkey),
        onStopAgent: (pubkey) => stopped.push(pubkey),
      }),
    ),
  );

  fireEvent.click(screen.getByRole("button", { name: "Fizz agent profile" }));
  fireEvent.click(screen.getByTestId(`agent-tile-stop-${FIRST_PK}`));
  fireEvent.click(screen.getByTestId(`agent-tile-add-to-channel-${FIRST_PK}`));
  assert.deepEqual(opened, [FIRST_PK]);
  assert.deepEqual(stopped, [FIRST_PK]);
  assert.deepEqual(added, [FIRST_PK]);
});

test("a preset card has no status, counts its agents and creates one", async () => {
  installIpc();
  const openedPersonas = [];
  const created = [];
  await renderWithClient(
    createElement(
      PresetsSection,
      presetsProps({
        agents: [agent(), agent({ pubkey: SECOND_PK, name: "Fizz 2" })],
        personas: [
          persona(),
          persona({ id: "persona-2", displayName: "Pollen" }),
        ],
        onCreateAgent: (preset) => created.push(preset.id),
        onOpenPersonaProfile: (preset) => openedPersonas.push(preset.id),
      }),
    ),
  );

  assert.ok(screen.getByText("used by 2 agents"));
  assert.ok(screen.getByText("not used yet"));
  assert.equal(screen.queryByText("Running"), null);
  assert.equal(screen.queryByText("Stopped"), null);

  fireEvent.click(screen.getByRole("button", { name: "Fizz preset" }));
  fireEvent.click(screen.getByTestId("preset-create-agent-persona-2"));
  assert.deepEqual(openedPersonas, ["persona-1"]);
  assert.deepEqual(created, ["persona-2"]);
});
