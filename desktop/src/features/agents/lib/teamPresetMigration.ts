import * as React from "react";
import { useQueryClient } from "@tanstack/react-query";

import { useManagedAgentsQuery } from "@/features/agents/hooks";
import {
  MY_TEAM_MAX_DESCRIPTION_CHARS,
  MY_TEAM_MAX_NAME_CHARS,
  type MyTeam,
  type MyTeamInput,
  saveMyTeam,
  useMyTeamsQuery,
} from "@/features/agents/lib/myTeams";
import { useTeamsQuery } from "@/features/agents/teamHooks";
import { useIdentityQuery } from "@/shared/api/hooks";
import type { AgentTeam, ManagedAgent } from "@/shared/api/types";
import { normalizePubkey } from "@/shared/lib/pubkey";

const MIGRATED_KEY_PREFIX = "kuzz.teamPresetsToTeams.v1.";

/** Owners whose migration is running, shared by every mounted page. */
const running = new Set<string>();

/**
 * Old teams were sets of agent presets; "Add to channel" on one made new
 * agents tagged with its id. They stay as team presets, and the agents made
 * from each become a real team of the same name. This computer's agents
 * only: every computer runs this for its own. A team with that name and
 * exactly those agents already (another run, another computer) is skipped.
 */
export function teamsFromTeamPresets(
  teamPresets: readonly AgentTeam[],
  agents: readonly ManagedAgent[],
  existing: readonly MyTeam[],
): MyTeamInput[] {
  const result: MyTeamInput[] = [];
  for (const preset of teamPresets) {
    const agentPubkeys = agents
      .filter((agent) => agent.teamId === preset.id)
      .map((agent) => normalizePubkey(agent.pubkey))
      .sort();
    if (agentPubkeys.length === 0) continue;
    const key = agentPubkeys.join(",");
    const duplicate = existing.some(
      (team) =>
        team.name === preset.name &&
        [...team.agentPubkeys].sort().join(",") === key,
    );
    if (duplicate) continue;
    result.push({
      name: [...preset.name].slice(0, MY_TEAM_MAX_NAME_CHARS).join(""),
      description: [...(preset.description ?? "")]
        .slice(0, MY_TEAM_MAX_DESCRIPTION_CHARS)
        .join(""),
      agentPubkeys,
    });
  }
  return result;
}

/**
 * Run the team-preset migration once per account on this computer, after the
 * team presets, this computer's agents and the existing teams have loaded.
 */
export function useTeamPresetMigration(): void {
  const queryClient = useQueryClient();
  const owner = normalizePubkey(useIdentityQuery().data?.pubkey ?? "");
  const presets = useTeamsQuery();
  const agents = useManagedAgentsQuery();
  const teams = useMyTeamsQuery();
  const ready =
    owner.length > 0 &&
    presets.isSuccess &&
    agents.isSuccess &&
    teams.isSuccess;

  React.useEffect(() => {
    if (!ready || running.has(owner)) return;
    const key = MIGRATED_KEY_PREFIX + owner;
    if (localStorage.getItem(key)) return;
    running.add(owner);
    const mine = (agents.data ?? []).filter(
      (agent) => normalizePubkey(agent.ownerPubkey ?? owner) === owner,
    );
    const inputs = teamsFromTeamPresets(
      presets.data ?? [],
      mine,
      teams.data ?? [],
    );
    void (async () => {
      try {
        for (const input of inputs) {
          await saveMyTeam(queryClient, owner, input);
        }
        localStorage.setItem(key, new Date().toISOString());
      } catch (error) {
        // Retried on the next app start.
        console.error("Could not make teams from team presets", error);
      } finally {
        running.delete(owner);
      }
    })();
  }, [agents.data, owner, presets.data, queryClient, ready, teams.data]);
}
