import { Plus, Trash2, Users } from "lucide-react";

import type { MyTeam } from "@/features/agents/lib/myTeams";
import { cn } from "@/shared/lib/cn";
import { Button } from "@/shared/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
} from "@/shared/ui/dropdown-menu";
import { Skeleton } from "@/shared/ui/skeleton";
import {
  AGENT_TILE_GRID_CLASS,
  DeviceHeading,
  TileMenuTrigger,
} from "./AgentTile";
import { useT } from "@/shared/i18n";

/** One of the owner's real agents, as a team shows and picks it. */
export type TeamAgent = {
  pubkey: string;
  name: string;
  /** The computer it runs on ("This device" for this one). */
  computer: string;
  running: boolean;
};

/**
 * The "Teams" part of My agents: each team is a group of the owner's real
 * agents (any computer) with each member's live status. Add to channel adds
 * those agents; nothing new is made.
 */
export function MyTeamsSection({
  agentsByPubkey,
  error,
  isLoading,
  teams,
  onAddToChannel,
  onCreate,
  onDelete,
  onEdit,
}: {
  agentsByPubkey: ReadonlyMap<string, TeamAgent>;
  error: Error | null;
  isLoading: boolean;
  teams: readonly MyTeam[];
  onAddToChannel: (team: MyTeam) => void;
  onCreate: () => void;
  onDelete: (team: MyTeam) => void;
  onEdit: (team: MyTeam) => void;
}) {
  const t = useT();
  return (
    <div className="space-y-3" data-testid="agents-my-teams">
      <DeviceHeading
        detail={t("agents.teamsSection.detail")}
        robot={<Users className="h-4 w-4 text-muted-foreground" />}
        title={t("agents.catalog.teams")}
      />
      {isLoading ? (
        <div className={AGENT_TILE_GRID_CLASS}>
          <Skeleton className="h-32 w-full rounded-xl" />
        </div>
      ) : (
        <div className={AGENT_TILE_GRID_CLASS}>
          {teams.map((team) => (
            <TeamTile
              agentsByPubkey={agentsByPubkey}
              key={team.id}
              team={team}
              onAddToChannel={onAddToChannel}
              onDelete={onDelete}
              onEdit={onEdit}
            />
          ))}
          <button
            className="flex min-h-32 items-center justify-center gap-1.5 rounded-xl border border-dashed border-border/80 text-sm font-medium text-muted-foreground transition-colors hover:border-border hover:bg-muted/60 hover:text-foreground focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring"
            data-testid="new-my-team-card"
            onClick={onCreate}
            type="button"
          >
            <Plus className="h-4 w-4" />
            {t("agents.myTeams.newTitle")}
          </button>
        </div>
      )}
      {error ? (
        <p className="rounded-2xl border border-destructive/30 bg-destructive/10 px-4 py-3 text-sm text-destructive">
          {error.message}
        </p>
      ) : null}
    </div>
  );
}

function TeamTile({
  agentsByPubkey,
  team,
  onAddToChannel,
  onDelete,
  onEdit,
}: {
  agentsByPubkey: ReadonlyMap<string, TeamAgent>;
  team: MyTeam;
  onAddToChannel: (team: MyTeam) => void;
  onDelete: (team: MyTeam) => void;
  onEdit: (team: MyTeam) => void;
}) {
  const t = useT();
  const members = team.agentPubkeys.map(
    (pubkey) => agentsByPubkey.get(pubkey) ?? null,
  );
  const present = members.filter(
    (member): member is TeamAgent => member !== null,
  );
  const missing = members.length - present.length;
  const computers = new Set(present.map((member) => member.computer));
  const summary = [
    t("agents.teamsSection.agentCount", { count: present.length }),
    computers.size > 1
      ? t("agents.teamsSection.computerCount", { count: computers.size })
      : null,
    missing > 0 ? t("agents.teamsSection.notFound", { count: missing }) : null,
  ]
    .filter(Boolean)
    .join(" · ");

  return (
    <div
      className="flex min-w-0 flex-col gap-3 rounded-xl border border-border/70 bg-background p-3 shadow-xs"
      data-testid={`my-team-${team.id}`}
    >
      <div className="flex min-w-0 items-start gap-2.5">
        <span className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-muted text-muted-foreground">
          <Users className="h-4 w-4" />
        </span>
        <div className="flex min-w-0 flex-1 flex-col">
          <span className="truncate text-sm font-semibold text-foreground">
            {team.name}
          </span>
          <span className="truncate text-xs text-muted-foreground">
            {summary}
          </span>
        </div>
        <span className="shrink-0 rounded-full bg-muted px-2 py-0.5 text-2xs font-medium text-muted-foreground">
          {t("agents.teamsSection.badge")}
        </span>
        <span className="-mr-1 -mt-1 shrink-0">
          <DropdownMenu modal={false}>
            <TileMenuTrigger
              label={t("agents.teamsSection.openActions", { name: team.name })}
            />
            <DropdownMenuContent
              align="end"
              onCloseAutoFocus={(event) => event.preventDefault()}
            >
              <DropdownMenuItem
                className="text-destructive focus:text-destructive"
                data-testid={`my-team-delete-${team.id}`}
                onClick={() => onDelete(team)}
              >
                <Trash2 className="h-4 w-4" />
                {t("agents.teamsSection.deleteTeam")}
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </span>
      </div>
      {present.length > 0 ? (
        <ul
          aria-label={t("agents.teamsSection.membersLabel", {
            name: team.name,
          })}
          className="flex min-h-8 flex-wrap content-start gap-1"
        >
          {present.map((member) => (
            <li
              className="flex max-w-full items-center gap-1 rounded-full bg-muted px-2 py-0.5 text-2xs text-foreground"
              key={member.pubkey}
              title={t("agents.teamsSection.memberTitle", {
                name: member.name,
                computer: member.computer,
                running: member.running ? "yes" : "no",
              })}
            >
              <span
                aria-hidden
                className={cn(
                  "h-1.5 w-1.5 shrink-0 rounded-full",
                  member.running ? "bg-emerald-500" : "bg-muted-foreground/40",
                )}
              />
              <span className="truncate">{member.name}</span>
            </li>
          ))}
        </ul>
      ) : (
        <p className="min-h-8 text-xs text-muted-foreground">
          {t("agents.teamsSection.empty")}
        </p>
      )}
      <div className="mt-auto flex flex-wrap gap-1.5">
        <Button
          data-testid={`my-team-add-to-channel-${team.id}`}
          disabled={present.length === 0}
          onClick={() => onAddToChannel(team)}
          size="xs"
        >
          {t("agents.addToGroup.add")}
        </Button>
        <Button
          data-testid={`my-team-edit-${team.id}`}
          onClick={() => onEdit(team)}
          size="xs"
          variant="outline"
        >
          {t("common.edit")}
        </Button>
      </div>
    </div>
  );
}
