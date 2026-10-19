/**
 * In-memory stand-in for the relay groups routes (`groups_request`) used by
 * the Playwright mock bridge. Seeds come from `mock.groups` in the E2E config;
 * without `groups` seeds the groups are the bridge's own mock channels, and
 * without `invitations` there are none.
 */

export type MockGroupSeed = {
  id: string;
  name: string;
  description?: string | null;
  handle?: string | null;
  channelType?: "stream" | "forum";
  memberCount?: number;
  isMember?: boolean;
  visibility?: "open" | "private";
  invitePolicy?: "members" | "admins";
};

export type MockGroupInvitationSeed = {
  id: string;
  groupId: string;
  inviter: string;
  createdAt?: string;
  /** Accepting answers 409 (the invitation is no longer valid). */
  expired?: boolean;
};

export type MockGroupsConfig = {
  groups?: MockGroupSeed[];
  /** Invite codes, keyed by code, pointing at a seeded group id. */
  inviteCodes?: Record<string, string>;
  invitations?: MockGroupInvitationSeed[];
  addPolicy?: "friends" | "everyone";
  /** Handles that answer 400 "this address is reserved". */
  reservedHandles?: string[];
  /** Host used in invite-link URLs. */
  host?: string;
};

type MockGroupsState = {
  groups: Map<string, Required<MockGroupSeed>>;
  inviteCodes: Map<string, string>;
  invitations: MockGroupInvitationSeed[];
  addPolicy: "friends" | "everyone";
  reserved: Set<string>;
  host: string;
};

let state: MockGroupsState | null = null;
let stateSource: MockGroupsConfig | undefined;

function httpError(status: number, message: string): Error {
  return new Error(`HTTP ${status}: ${message}`);
}

function fromSeed(seed: MockGroupSeed): Required<MockGroupSeed> {
  return {
    id: seed.id,
    name: seed.name,
    description: seed.description ?? null,
    handle: seed.handle ?? null,
    channelType: seed.channelType ?? "stream",
    memberCount: seed.memberCount ?? 1,
    isMember: seed.isMember ?? false,
    visibility: seed.visibility ?? "open",
    invitePolicy: seed.invitePolicy ?? "members",
  };
}

function getState(
  config: MockGroupsConfig | undefined,
  fallbackGroups: () => MockGroupSeed[],
): MockGroupsState {
  if (!state || stateSource !== config) {
    stateSource = config;
    state = {
      groups: new Map(
        (config?.groups ?? []).map((seed) => [seed.id, fromSeed(seed)]),
      ),
      inviteCodes: new Map(Object.entries(config?.inviteCodes ?? {})),
      invitations: [...(config?.invitations ?? [])],
      addPolicy: config?.addPolicy ?? "friends",
      reserved: new Set(config?.reservedHandles ?? []),
      host: config?.host ?? "kuzz.example",
    };
  }
  if (!config?.groups) {
    // Track the bridge's live channels (joins, renames), keeping any address
    // or invite policy set through these routes.
    for (const seed of fallbackGroups()) {
      const current = state.groups.get(seed.id);
      state.groups.set(seed.id, {
        ...fromSeed(seed),
        handle: current?.handle ?? null,
        invitePolicy: current?.invitePolicy ?? "members",
      });
    }
  }
  return state;
}

function summary(group: Required<MockGroupSeed>) {
  const { visibility: _v, invitePolicy: _p, ...item } = group;
  return item;
}

function requireGroup(s: MockGroupsState, id: string) {
  const group = s.groups.get(decodeURIComponent(id));
  if (!group) throw httpError(404, "group not found");
  return group;
}

function codeFor(s: MockGroupsState, groupId: string): string {
  for (const [code, id] of s.inviteCodes) if (id === groupId) return code;
  const code = `mockInviteCode${groupId.replace(/[^A-Za-z0-9]/g, "")}`.slice(
    0,
    40,
  );
  const padded = code.padEnd(22, "x");
  s.inviteCodes.set(padded, groupId);
  return padded;
}

function joinResult(group: Required<MockGroupSeed>) {
  const joined = !group.isMember;
  if (joined) {
    group.isMember = true;
    group.memberCount += 1;
  }
  return { channelId: group.id, joined };
}

/** A bridge mock channel as a groups seed. */
export function mockGroupSeedFromChannel(channel: {
  id: string;
  name: string;
  description: string;
  channel_type: string;
  visibility: "open" | "private";
  member_count: number;
  is_member: boolean;
  archived_at: string | null;
}): MockGroupSeed | null {
  if (channel.channel_type === "dm" || channel.archived_at) return null;
  return {
    id: channel.id,
    name: channel.name,
    description: channel.description || null,
    channelType: channel.channel_type === "forum" ? "forum" : "stream",
    memberCount: channel.member_count,
    isMember: channel.is_member,
    visibility: channel.visibility,
  };
}

export function handleMockGroupsRequest(
  config: MockGroupsConfig | undefined,
  payload: { method: string; path: string; body: unknown },
  fallbackGroups: () => MockGroupSeed[] = () => [],
): unknown {
  const s = getState(config, fallbackGroups);
  const [pathname, search = ""] = payload.path.split("?");
  const parts = pathname.split("/").filter(Boolean); // ["api", ...]
  const method = payload.method.toUpperCase();

  if (pathname === "/api/me/group-add-policy") {
    if (method === "PUT") {
      s.addPolicy = (payload.body as { policy: "friends" | "everyone" }).policy;
    }
    return { policy: s.addPolicy };
  }

  if (parts[1] === "group-invitations") {
    if (parts.length === 2) {
      return s.invitations.flatMap((invitation) => {
        const group = s.groups.get(invitation.groupId);
        if (!group) return [];
        return [
          {
            id: invitation.id,
            channel: {
              id: group.id,
              name: group.name,
              description: group.description,
              channelType: group.channelType,
              memberCount: group.memberCount,
              visibility: group.visibility,
            },
            inviter: invitation.inviter,
            createdAt: invitation.createdAt ?? new Date().toISOString(),
          },
        ];
      });
    }
    const index = s.invitations.findIndex((item) => item.id === parts[2]);
    if (index < 0) throw httpError(404, "invitation not found");
    const [invitation] = s.invitations.splice(index, 1);
    if (parts[3] === "decline") return null;
    if (invitation.expired)
      throw httpError(409, "invitation is no longer valid");
    return joinResult(requireGroup(s, invitation.groupId));
  }

  // /api/groups/...
  if (parts[2] === "search") {
    const q = (new URLSearchParams(search).get("q") ?? "").toLowerCase();
    const groups = [...s.groups.values()]
      .filter(
        (group) =>
          group.visibility === "open" &&
          [group.name, group.description ?? "", group.handle ?? ""].some(
            (text) => text.toLowerCase().includes(q),
          ),
      )
      .sort((a, b) => b.memberCount - a.memberCount)
      .slice(0, 20)
      .map(summary);
    return { groups };
  }
  if (parts[2] === "by-handle") {
    const handle = decodeURIComponent(parts[3] ?? "").toLowerCase();
    const group = [...s.groups.values()].find(
      (item) => item.handle === handle && item.visibility === "open",
    );
    if (!group) throw httpError(404, "group not found");
    return summary(group);
  }
  if (parts[2] === "invite") {
    const groupId = s.inviteCodes.get(decodeURIComponent(parts[3] ?? ""));
    if (!groupId) throw httpError(404, "invite link not found");
    const group = requireGroup(s, groupId);
    if (parts[4] === "join") return joinResult(group);
    const { handle: _h, invitePolicy: _p, ...preview } = group;
    return preview;
  }

  const group = requireGroup(s, parts[2] ?? "");
  switch (parts[3]) {
    case undefined:
      return { ...group };
    case "handle": {
      const raw = (payload.body as { handle: string | null }).handle;
      const handle = raw ? raw.trim().replace(/^@/, "").toLowerCase() : null;
      if (handle !== null) {
        if (s.reserved.has(handle)) {
          throw httpError(400, "this address is reserved");
        }
        if (!/^[a-z0-9_]{3,32}$/.test(handle)) {
          throw httpError(400, "invalid address");
        }
        const taken = [...s.groups.values()].some(
          (other) => other.id !== group.id && other.handle === handle,
        );
        if (taken) throw httpError(409, "this address is taken");
      }
      group.handle = handle;
      return { ...group };
    }
    case "invite-link": {
      if (parts[4] === "reset") {
        for (const [code, id] of s.inviteCodes) {
          if (id === group.id) s.inviteCodes.delete(code);
        }
        const code =
          `reset${Date.now().toString(36)}${group.id.replace(/[^A-Za-z0-9]/g, "")}`
            .slice(0, 40)
            .padEnd(22, "x");
        s.inviteCodes.set(code, group.id);
        return { code, url: `https://${s.host}/g/${code}` };
      }
      const code = codeFor(s, group.id);
      return { code, url: `https://${s.host}/g/${code}` };
    }
    case "invite-policy": {
      if (method === "PUT") {
        group.invitePolicy = (
          payload.body as { policy: "members" | "admins" }
        ).policy;
      }
      return { policy: group.invitePolicy };
    }
    default:
      throw httpError(404, "not found");
  }
}
