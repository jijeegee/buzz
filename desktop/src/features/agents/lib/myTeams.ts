import * as React from "react";
import {
  type QueryClient,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";

import { useIdentityQuery } from "@/shared/api/hooks";
import { relayClient } from "@/shared/api/relayClient";
import { meLive } from "@/shared/api/relayMeLive";
import { signRelayEvent } from "@/shared/api/tauri";
import type { RelayEvent } from "@/shared/api/types";
import { KIND_AGENT_TEAM, KIND_DELETION } from "@/shared/constants/kinds";
import { normalizePubkey } from "@/shared/lib/pubkey";
import { t } from "@/shared/i18n";

/**
 * A team: a group of the owner's real agents, which may run on different
 * computers (kind:30182). Adding a team to a channel adds these agents; it
 * never makes new ones. Not to be confused with a team preset (`AgentTeam`,
 * kind:30176), a set of agent presets used to set up a computer.
 */
export type MyTeam = {
  id: string;
  name: string;
  description: string | null;
  /** Lowercase hex pubkeys of the team's agents, in the owner's order. */
  agentPubkeys: string[];
  createdAt: number;
};

export type MyTeamInput = {
  /** Omitted for a new team. */
  id?: string;
  name: string;
  description?: string | null;
  agentPubkeys: readonly string[];
};

/** The relay refuses more; see `validate_agent_team`. */
export const MY_TEAM_MAX_AGENTS = 64;
/** Longest name and description, in characters; the relay refuses more. */
export const MY_TEAM_MAX_NAME_CHARS = 200;
export const MY_TEAM_MAX_DESCRIPTION_CHARS = 2_000;

const MY_TEAMS_QUERY_KEY = ["my-teams"] as const;
const MY_TEAMS_FETCH_LIMIT = 500;
const HEX_PUBKEY = /^[0-9a-f]{64}$/;

/** The query client is per community, so the key only needs the owner. */
export function myTeamsQueryKey(ownerPubkey: string) {
  return [...MY_TEAMS_QUERY_KEY, normalizePubkey(ownerPubkey)] as const;
}

/** The event content of a team. */
export function myTeamContent(input: MyTeamInput): string {
  const description = input.description?.trim();
  return JSON.stringify({
    v: 1,
    name: input.name.trim(),
    ...(description ? { description } : {}),
    agent_pubkeys: uniqueAgentPubkeys(input.agentPubkeys),
  });
}

function uniqueAgentPubkeys(pubkeys: readonly string[]): string[] {
  const seen = new Set<string>();
  const result: string[] = [];
  for (const raw of pubkeys) {
    const pubkey = normalizePubkey(raw);
    if (!HEX_PUBKEY.test(pubkey) || seen.has(pubkey)) continue;
    seen.add(pubkey);
    result.push(pubkey);
  }
  return result;
}

/**
 * The owner's teams from their kind:30182 events: the newest event per team
 * id, skipping ones that are not theirs or do not parse. Sorted by name.
 */
export function myTeamsFromEvents(
  events: readonly RelayEvent[],
  ownerPubkey: string,
): MyTeam[] {
  const owner = normalizePubkey(ownerPubkey);
  const newest = new Map<string, RelayEvent>();
  for (const event of events) {
    if (event.kind !== KIND_AGENT_TEAM) continue;
    if (normalizePubkey(event.pubkey) !== owner) continue;
    const id = event.tags.find((tag) => tag[0] === "d")?.[1];
    if (!id) continue;
    const current = newest.get(id);
    if (!current || event.created_at > current.created_at) {
      newest.set(id, event);
    }
  }
  const teams: MyTeam[] = [];
  for (const [id, event] of newest) {
    let content: unknown;
    try {
      content = JSON.parse(event.content);
    } catch {
      continue;
    }
    if (typeof content !== "object" || content === null) continue;
    const { name, description, agent_pubkeys } = content as Record<
      string,
      unknown
    >;
    if (typeof name !== "string" || name.trim().length === 0) continue;
    teams.push({
      id,
      name: name.trim(),
      description:
        typeof description === "string" && description.trim()
          ? description.trim()
          : null,
      agentPubkeys: Array.isArray(agent_pubkeys)
        ? uniqueAgentPubkeys(
            agent_pubkeys.filter(
              (pubkey): pubkey is string => typeof pubkey === "string",
            ),
          )
        : [],
      createdAt: event.created_at,
    });
  }
  return teams.sort((a, b) =>
    a.name.localeCompare(b.name, undefined, { sensitivity: "base" }),
  );
}

export async function fetchMyTeams(ownerPubkey: string): Promise<MyTeam[]> {
  const owner = normalizePubkey(ownerPubkey);
  const events = await relayClient.fetchEvents({
    kinds: [KIND_AGENT_TEAM],
    authors: [owner],
    limit: MY_TEAMS_FETCH_LIMIT,
  });
  return myTeamsFromEvents(events, owner);
}

/** The signed-in owner's teams; live updates come from `useMyTeamsLiveUpdates`. */
export function useMyTeamsQuery() {
  const owner = normalizePubkey(useIdentityQuery().data?.pubkey ?? "");
  return useQuery({
    enabled: owner.length > 0,
    queryKey: myTeamsQueryKey(owner),
    queryFn: () => fetchMyTeams(owner),
    staleTime: 60_000,
  });
}

/** Whether [event] is a deletion of one of [owner]'s teams. */
export function isMyTeamDeletion(event: RelayEvent, owner: string): boolean {
  const prefix = `${KIND_AGENT_TEAM}:${owner}:`;
  return (
    event.kind === KIND_DELETION &&
    event.tags.some((tag) => tag[0] === "a" && tag[1]?.startsWith(prefix))
  );
}

/**
 * Keep the owner's teams fresh: live subscriptions to their kind:30182
 * events and to their deletions of them (a team edited or deleted on
 * another computer, the phone or in the CLI), plus a refetch after
 * reconnect.
 */
export function useMyTeamsLiveUpdates(): void {
  const queryClient = useQueryClient();
  const owner = normalizePubkey(useIdentityQuery().data?.pubkey ?? "");

  React.useEffect(() => {
    if (!owner) return;
    const queryKey = myTeamsQueryKey(owner);
    const invalidate = () => {
      void queryClient.invalidateQueries({ queryKey });
    };
    let disposed = false;
    const disposers: Array<() => void> = [];
    const subscribe = (
      filter: Parameters<typeof meLive.subscribe>[0],
      onEvent: (event: RelayEvent) => void,
    ) => {
      void meLive
        .subscribe(filter, onEvent)
        .then((unsubscribe) => {
          if (disposed) void unsubscribe();
          else
            disposers.push(() => {
              void unsubscribe();
            });
        })
        .catch((error) => {
          console.error("Failed to subscribe to teams", error);
        });
    };
    subscribe(
      { kinds: [KIND_AGENT_TEAM], authors: [owner], limit: 0 },
      invalidate,
    );
    subscribe(
      { kinds: [KIND_DELETION], authors: [owner], limit: 0 },
      (event) => {
        if (isMyTeamDeletion(event, owner)) invalidate();
      },
    );
    const unsubscribeReconnect = relayClient.subscribeToReconnects(invalidate);
    return () => {
      disposed = true;
      unsubscribeReconnect();
      for (const dispose of disposers) dispose();
    };
  }, [owner, queryClient]);
}

export type MyTeamsWriteDeps = {
  sign: typeof signRelayEvent;
  publish: (event: RelayEvent) => Promise<unknown>;
  now: () => number;
  newId: () => string;
};

const defaultWriteDeps: MyTeamsWriteDeps = {
  sign: signRelayEvent,
  publish: (event) =>
    relayClient.publishEvent(
      event,
      t("agents.myTeams.saveTimedOut"),
      t("agents.myTeams.saveFailed"),
    ),
  now: () => Date.now(),
  newId: () => crypto.randomUUID(),
};

/**
 * Save a team (new or edited) as a kind:30182 event and put it in the cached
 * list. Returns the saved team.
 */
export async function saveMyTeam(
  queryClient: QueryClient,
  ownerPubkey: string,
  input: MyTeamInput,
  deps: MyTeamsWriteDeps = defaultWriteDeps,
): Promise<MyTeam> {
  const name = input.name.trim();
  if (!name) throw new Error(t("agents.myTeams.nameRequired"));
  if ([...name].length > MY_TEAM_MAX_NAME_CHARS) {
    throw new Error(
      t("agents.myTeams.nameTooLong", { max: MY_TEAM_MAX_NAME_CHARS }),
    );
  }
  if (
    [...(input.description?.trim() ?? "")].length >
    MY_TEAM_MAX_DESCRIPTION_CHARS
  ) {
    throw new Error(
      t("agents.myTeams.descriptionTooLong", {
        max: MY_TEAM_MAX_DESCRIPTION_CHARS,
      }),
    );
  }
  const agentPubkeys = uniqueAgentPubkeys(input.agentPubkeys);
  if (agentPubkeys.length > MY_TEAM_MAX_AGENTS) {
    throw new Error(
      t("agents.myTeams.tooManyAgents", { max: MY_TEAM_MAX_AGENTS }),
    );
  }
  const queryKey = myTeamsQueryKey(ownerPubkey);
  const previous = queryClient
    .getQueryData<MyTeam[]>(queryKey)
    ?.find((team) => team.id === input.id);
  const id = input.id ?? deps.newId();
  // Replaceable events keep the newest; an edit within the same second must
  // still supersede the previous version.
  const createdAt = Math.max(
    Math.floor(deps.now() / 1_000),
    (previous?.createdAt ?? 0) + 1,
  );
  const event = await deps.sign({
    kind: KIND_AGENT_TEAM,
    content: myTeamContent({ ...input, agentPubkeys }),
    createdAt,
    tags: [["d", id]],
  });
  await deps.publish(event);
  const [team] = myTeamsFromEvents([event], ownerPubkey);
  if (!team) throw new Error(t("agents.myTeams.saveFailed"));
  await queryClient.cancelQueries({ queryKey });
  queryClient.setQueryData<MyTeam[]>(queryKey, (current) =>
    [...(current ?? []).filter((other) => other.id !== id), team].sort((a, b) =>
      a.name.localeCompare(b.name, undefined, { sensitivity: "base" }),
    ),
  );
  return team;
}

/** Delete a team (its agents stay) with a NIP-09 coordinate deletion. */
export async function deleteMyTeam(
  queryClient: QueryClient,
  ownerPubkey: string,
  team: Pick<MyTeam, "id" | "name" | "createdAt">,
  deps: MyTeamsWriteDeps = defaultWriteDeps,
): Promise<void> {
  const owner = normalizePubkey(ownerPubkey);
  const event = await deps.sign({
    kind: KIND_DELETION,
    content: `Delete team ${team.name}`,
    createdAt: Math.max(Math.floor(deps.now() / 1_000), team.createdAt + 1),
    tags: [["a", `${KIND_AGENT_TEAM}:${owner}:${team.id}`]],
  });
  await deps.publish(event);
  const queryKey = myTeamsQueryKey(owner);
  await queryClient.cancelQueries({ queryKey });
  queryClient.setQueryData<MyTeam[]>(queryKey, (current) =>
    (current ?? []).filter((other) => other.id !== team.id),
  );
}
