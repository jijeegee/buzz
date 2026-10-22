import { invokeTauri } from "@/shared/api/tauri";

/**
 * Groups routes on the active community's server (relay PR #98): search,
 * public address, invite links, who-can-invite, who-can-add-me and
 * invitations. Field names match the server JSON (camelCase).
 */

export type GroupChannelType = "stream" | "forum";
export type GroupVisibility = "open" | "private";
export type GroupInvitePolicy = "members" | "admins";
export type GroupAddPolicy = "friends" | "everyone";

/** One group from search, by-handle, or `GET /api/groups/{id}`. */
export type GroupSummary = {
  id: string;
  name: string;
  description: string | null;
  handle: string | null;
  channelType: GroupChannelType;
  memberCount: number;
  isMember: boolean;
  visibility?: GroupVisibility;
  invitePolicy?: GroupInvitePolicy;
  /** Group photo URL; null/absent = none (older servers omit it). */
  picture?: string | null;
};

/** `GET /api/groups/invite/{code}`: no handle; private groups included. */
export type GroupInvitePreview = {
  id: string;
  name: string;
  description: string | null;
  channelType: GroupChannelType;
  memberCount: number;
  visibility: GroupVisibility;
  isMember: boolean;
  picture?: string | null;
};

export type GroupInviteLink = { code: string; url: string };
export type GroupJoinResult = { channelId: string; joined: boolean };

export type GroupInvitation = {
  id: string;
  channel: {
    id: string;
    name: string;
    description: string | null;
    channelType: GroupChannelType;
    memberCount: number;
    visibility: GroupVisibility;
    picture?: string | null;
    /** Ownerless chat: drawn from its people, titled by their names. */
    groupChat?: boolean;
    /** Up to four active members (you excluded), for a chat's picture. */
    memberPubkeys?: string[];
  };
  /** Inviter pubkey (hex). */
  inviter: string;
  createdAt: string;
};

/** Search needs 2+ characters and returns at most 20 groups. */
export const GROUP_SEARCH_MIN_LENGTH = 2;
export const GROUP_SEARCH_LIMIT = 20;
export const GROUP_SEARCH_DEBOUNCE_MS = 250;

function request<T>(
  method: "GET" | "POST" | "PUT",
  path: string,
  body?: unknown,
): Promise<T> {
  return invokeTauri<T>("groups_request", {
    method,
    path,
    body: body === undefined ? null : body,
  });
}

const segment = encodeURIComponent;

/**
 * The HTTP status of a failed groups call (`HTTP 409: ...` from the native
 * command), or null for a network or other failure.
 */
export function groupsErrorStatus(error: unknown): number | null {
  const text =
    error instanceof Error
      ? error.message
      : typeof error === "string"
        ? error
        : "";
  const match = /^HTTP (\d{3})\b/.exec(text);
  return match ? Number(match[1]) : null;
}

/** The server's error text without the `HTTP nnn:` prefix. */
export function groupsErrorText(error: unknown): string {
  const text =
    error instanceof Error
      ? error.message
      : typeof error === "string"
        ? error
        : "";
  return text.replace(/^HTTP \d{3}:?\s*/, "");
}

export async function searchGroups(query: string): Promise<GroupSummary[]> {
  const q = query.trim();
  const result = await request<{ groups: GroupSummary[] }>(
    "GET",
    `/api/groups/search?q=${segment(q)}&limit=${GROUP_SEARCH_LIMIT}`,
  );
  return result.groups ?? [];
}

export function getGroupByHandle(handle: string): Promise<GroupSummary> {
  return request("GET", `/api/groups/by-handle/${segment(handle)}`);
}

export function getGroup(groupId: string): Promise<GroupSummary> {
  return request("GET", `/api/groups/${segment(groupId)}`);
}

export function setGroupHandle(
  groupId: string,
  handle: string | null,
): Promise<GroupSummary> {
  return request("PUT", `/api/groups/${segment(groupId)}/handle`, { handle });
}

export function getGroupInviteLink(groupId: string): Promise<GroupInviteLink> {
  return request("GET", `/api/groups/${segment(groupId)}/invite-link`);
}

export function resetGroupInviteLink(
  groupId: string,
): Promise<GroupInviteLink> {
  return request(
    "POST",
    `/api/groups/${segment(groupId)}/invite-link/reset`,
    {},
  );
}

export function previewGroupInvite(code: string): Promise<GroupInvitePreview> {
  return request("GET", `/api/groups/invite/${segment(code)}`);
}

export function joinGroupByInvite(code: string): Promise<GroupJoinResult> {
  return request("POST", `/api/groups/invite/${segment(code)}/join`, {});
}

export async function setGroupInvitePolicy(
  groupId: string,
  policy: GroupInvitePolicy,
): Promise<GroupInvitePolicy> {
  const result = await request<{ policy: GroupInvitePolicy }>(
    "PUT",
    `/api/groups/${segment(groupId)}/invite-policy`,
    { policy },
  );
  return result.policy;
}

export async function getGroupAddPolicy(): Promise<GroupAddPolicy> {
  const result = await request<{ policy: GroupAddPolicy }>(
    "GET",
    "/api/me/group-add-policy",
  );
  return result.policy;
}

export async function setGroupAddPolicy(
  policy: GroupAddPolicy,
): Promise<GroupAddPolicy> {
  const result = await request<{ policy: GroupAddPolicy }>(
    "PUT",
    "/api/me/group-add-policy",
    { policy },
  );
  return result.policy;
}

export async function listGroupInvitations(): Promise<GroupInvitation[]> {
  const result = await request<GroupInvitation[] | null>(
    "GET",
    "/api/group-invitations",
  );
  return Array.isArray(result) ? result : [];
}

export function acceptGroupInvitation(id: string): Promise<GroupJoinResult> {
  return request("POST", `/api/group-invitations/${segment(id)}/accept`, {});
}

export function declineGroupInvitation(id: string): Promise<void> {
  return request("POST", `/api/group-invitations/${segment(id)}/decline`, {});
}
