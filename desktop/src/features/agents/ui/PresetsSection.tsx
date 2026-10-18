import * as React from "react";
import {
  CopyPlus,
  EllipsisVertical,
  Plus,
  Share2,
  Trash2,
  Upload,
} from "lucide-react";

import { effectiveAgentDescription } from "@/features/agents/lib/agentDescription";
import { presetUsageLabel } from "@/features/agents/lib/myAgentDevices";
import { resolveTeamPersonas } from "@/features/agents/lib/teamPersonas";
import { useRemoteOwnedAgents } from "@/features/agents/useRemoteOwnedAgents";
import { useIsArchivedPredicate } from "@/features/identity-archive/hooks";
import type { AgentPersona, AgentTeam, ManagedAgent } from "@/shared/api/types";
import { Button } from "@/shared/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/shared/ui/dropdown-menu";
import { Skeleton } from "@/shared/ui/skeleton";
import { AGENT_TILE_GRID_CLASS, AgentTile } from "./AgentTile";

type PresetsSectionProps = {
  agents: ManagedAgent[];
  isActionPending: boolean;
  isLoading: boolean;
  personas: AgentPersona[];
  personasError: Error | null;
  startingPersonaIds: ReadonlySet<string>;
  teams: AgentTeam[];
  teamsError: Error | null;
  isTeamsLoading: boolean;
  isTeamsPending: boolean;
  onCreateAgent: (persona: AgentPersona) => void;
  onDeactivatePersona: (persona: AgentPersona) => void;
  onDeletePersona: (persona: AgentPersona) => void;
  onDuplicatePersona: (persona: AgentPersona) => void;
  onEditPersona: (persona: AgentPersona) => void;
  onImportPreset: () => void;
  onOpenPersonaProfile: (persona: AgentPersona) => void;
  onSharePersona: (persona: AgentPersona) => void;
  onAddTeamToChannel: (team: AgentTeam) => void;
  onCreateTeam: () => void;
  onDeleteTeam: (team: AgentTeam) => void;
  onDuplicateTeam: (team: AgentTeam) => void;
  onEditTeam: (team: AgentTeam) => void;
  onImportTeam: () => void;
  onShareTeam: (team: AgentTeam) => void;
};

function SectionTitle({
  action,
  description,
  title,
}: {
  action?: React.ReactNode;
  description: string;
  title: string;
}) {
  return (
    <div className="flex items-start justify-between gap-3">
      <div className="min-w-0 space-y-0.5">
        <h2 className="text-sm font-semibold text-foreground">{title}</h2>
        <p className="text-xs text-muted-foreground">{description}</p>
      </div>
      {action}
    </div>
  );
}

function MenuTrigger({ label }: { label: string }) {
  return (
    <DropdownMenuTrigger asChild>
      <button
        aria-label={label}
        className="flex h-7 w-7 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
        type="button"
      >
        <EllipsisVertical className="h-4 w-4" />
      </button>
    </DropdownMenuTrigger>
  );
}

/**
 * The Agents page's "Presets" tab: templates with no status. An agent preset
 * makes a new agent on this computer ("Create agent"); a team preset is a
 * set of agent presets.
 */
export function PresetsSection(props: PresetsSectionProps) {
  const {
    agents,
    isActionPending,
    isLoading,
    personas,
    personasError,
    startingPersonaIds,
    teams,
    teamsError,
    isTeamsLoading,
    isTeamsPending,
  } = props;
  const isArchived = useIsArchivedPredicate();
  const { agents: remoteAgents } = useRemoteOwnedAgents();
  // Agents on every computer, like the phone's count.
  const usage = React.useMemo(() => {
    const counts = new Map<string, number>();
    const count = (personaId: string | null, pubkey: string) => {
      if (!personaId || isArchived(pubkey)) return;
      counts.set(personaId, (counts.get(personaId) ?? 0) + 1);
    };
    for (const agent of agents) count(agent.personaId, agent.pubkey);
    for (const agent of remoteAgents) count(agent.localPersonaId, agent.pubkey);
    return counts;
  }, [agents, isArchived, remoteAgents]);

  return (
    <div className="space-y-8" data-testid="agents-presets">
      <section className="space-y-3" data-testid="agents-library-personas">
        <SectionTitle
          action={
            <Button
              data-testid="agents-import-preset"
              onClick={props.onImportPreset}
              size="sm"
              variant="outline"
            >
              <Upload />
              Import preset
            </Button>
          }
          description='A preset is a template. It has no status. "Create agent" makes a new agent on this computer.'
          title="Agent presets"
        />
        {isLoading ? (
          <TilesSkeleton />
        ) : personas.length === 0 ? (
          <p className="text-sm text-muted-foreground">No presets yet.</p>
        ) : (
          <div className={AGENT_TILE_GRID_CLASS}>
            {personas.map((persona) => (
              <AgentPresetTile
                {...props}
                agentCount={usage.get(persona.id) ?? 0}
                isCreating={startingPersonaIds.has(persona.id)}
                isPending={isActionPending}
                key={persona.id}
                persona={persona}
              />
            ))}
          </div>
        )}
        {personasError ? <ErrorNote error={personasError} /> : null}
      </section>

      <section className="space-y-3" data-testid="agents-library-teams">
        <SectionTitle
          description="A team preset is a set of agent presets."
          title="Team presets"
        />
        {isTeamsLoading ? (
          <TilesSkeleton />
        ) : (
          <div className={AGENT_TILE_GRID_CLASS}>
            {teams.map((team) => (
              <TeamPresetTile
                {...props}
                isPending={isTeamsPending}
                key={team.id}
                team={team}
              />
            ))}
            <NewTeamPresetTile
              isPending={isTeamsPending}
              onCreate={props.onCreateTeam}
              onImport={props.onImportTeam}
            />
          </div>
        )}
        {teamsError ? <ErrorNote error={teamsError} /> : null}
      </section>
    </div>
  );
}

function AgentPresetTile({
  agentCount,
  isCreating,
  isPending,
  persona,
  onCreateAgent,
  onDeactivatePersona,
  onDeletePersona,
  onDuplicatePersona,
  onEditPersona,
  onOpenPersonaProfile,
  onSharePersona,
}: PresetsSectionProps & {
  agentCount: number;
  isCreating: boolean;
  isPending: boolean;
  persona: AgentPersona;
}) {
  // A team's own presets are managed by that team.
  const canEdit = !persona.sourceTeam;
  return (
    <AgentTile
      actions={
        <DropdownMenu modal={false}>
          <MenuTrigger label={`Open actions for ${persona.displayName}`} />
          <DropdownMenuContent
            align="end"
            onCloseAutoFocus={(event) => event.preventDefault()}
          >
            <DropdownMenuItem
              disabled={isPending}
              onClick={() => onDuplicatePersona(persona)}
            >
              <CopyPlus className="h-4 w-4" />
              Duplicate
            </DropdownMenuItem>
            <DropdownMenuItem
              disabled={isPending}
              onClick={() => onSharePersona(persona)}
            >
              <Share2 className="h-4 w-4" />
              Share
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            {persona.sourceTeam ? (
              <DropdownMenuItem disabled>
                <Trash2 className="h-4 w-4" />
                Managed by team
              </DropdownMenuItem>
            ) : (
              <DropdownMenuItem
                className="text-destructive focus:text-destructive"
                disabled={isPending}
                onClick={() =>
                  persona.isBuiltIn
                    ? onDeactivatePersona(persona)
                    : onDeletePersona(persona)
                }
              >
                <Trash2 className="h-4 w-4" />
                Delete
              </DropdownMenuItem>
            )}
          </DropdownMenuContent>
        </DropdownMenu>
      }
      avatarUrl={persona.avatarUrl}
      badge="Preset"
      buttons={
        <>
          <Button
            data-testid={`preset-create-agent-${persona.id}`}
            disabled={isPending || isCreating}
            onClick={() => onCreateAgent(persona)}
            size="xs"
          >
            {isCreating ? "Creating…" : "Create agent"}
          </Button>
          {canEdit ? (
            <Button
              data-testid={`preset-edit-${persona.id}`}
              disabled={isPending}
              onClick={() => onEditPersona(persona)}
              size="xs"
              variant="outline"
            >
              Edit preset
            </Button>
          ) : null}
        </>
      }
      dataTestId={`persona-agent-row-${persona.id}`}
      description={effectiveAgentDescription(persona)}
      label={persona.displayName}
      openLabel={`${persona.displayName} preset`}
      subtitle={presetUsageLabel(agentCount)}
      // A persona target, never an agent pubkey: the panel resolves it to a
      // live agent made from it on every render.
      onOpen={() => onOpenPersonaProfile(persona)}
    />
  );
}

function TeamPresetTile({
  isPending,
  personas,
  team,
  onAddTeamToChannel,
  onDeleteTeam,
  onDuplicateTeam,
  onEditTeam,
  onShareTeam,
}: PresetsSectionProps & { isPending: boolean; team: AgentTeam }) {
  const resolution = resolveTeamPersonas(team, personas);
  const missing = resolution.missingPersonaCount;
  const names = resolution.resolvedPersonas.map(
    (persona) => persona.displayName,
  );
  const count = team.personaIds.length;
  return (
    <AgentTile
      actions={
        <DropdownMenu modal={false}>
          <MenuTrigger label={`${team.name} team actions`} />
          <DropdownMenuContent
            align="end"
            onCloseAutoFocus={(event) => event.preventDefault()}
          >
            <DropdownMenuItem
              disabled={isPending || resolution.hasMissingPersonas}
              onClick={() => onDuplicateTeam(team)}
            >
              <CopyPlus className="h-4 w-4" />
              Duplicate
            </DropdownMenuItem>
            <DropdownMenuItem
              disabled={isPending || resolution.hasMissingPersonas}
              onClick={() => onShareTeam(team)}
            >
              <Share2 className="h-4 w-4" />
              Share
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem
              className="text-destructive focus:text-destructive"
              disabled={isPending}
              onClick={() => onDeleteTeam(team)}
            >
              <Trash2 className="h-4 w-4" />
              Delete
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      }
      avatarUrl={null}
      badge="Team preset"
      buttons={
        <>
          <Button
            data-testid={`team-add-to-channel-${team.id}`}
            disabled={isPending || resolution.hasMissingPersonas}
            onClick={() => onAddTeamToChannel(team)}
            size="xs"
          >
            Add to channel
          </Button>
          <Button
            disabled={isPending}
            onClick={() => onEditTeam(team)}
            size="xs"
            variant="outline"
          >
            Edit preset
          </Button>
        </>
      }
      dataTestId={`team-card-${team.id}`}
      description={
        missing > 0
          ? `${missing} preset${missing === 1 ? " is" : "s are"} missing. Edit the team to fix it.`
          : names.join(" · ") || team.description
      }
      label={team.name}
      openLabel={`${team.name} team preset`}
      subtitle={count === 1 ? "1 preset" : `${count} presets`}
      onOpen={() => onEditTeam(team)}
    />
  );
}

function NewTeamPresetTile({
  isPending,
  onCreate,
  onImport,
}: {
  isPending: boolean;
  onCreate: () => void;
  onImport: () => void;
}) {
  return (
    <DropdownMenu modal={false}>
      <DropdownMenuTrigger asChild>
        <button
          className="flex min-h-32 items-center justify-center gap-1.5 rounded-xl border border-dashed border-border/80 text-sm font-medium text-muted-foreground transition-colors hover:border-border hover:bg-muted/60 hover:text-foreground focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring"
          data-testid="new-team-card"
          disabled={isPending}
          type="button"
        >
          <Plus className="h-4 w-4" />
          New team preset
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        align="start"
        onCloseAutoFocus={(event) => event.preventDefault()}
      >
        <DropdownMenuItem onClick={onCreate}>
          Create team preset
        </DropdownMenuItem>
        <DropdownMenuItem onClick={onImport}>Import</DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function ErrorNote({ error }: { error: Error }) {
  return (
    <p className="rounded-2xl border border-destructive/30 bg-destructive/10 px-4 py-3 text-sm text-destructive">
      {error.message}
    </p>
  );
}

function TilesSkeleton() {
  return (
    <div className={AGENT_TILE_GRID_CLASS}>
      {[1, 2, 3].map((key) => (
        <Skeleton className="h-32 w-full rounded-xl" key={key} />
      ))}
    </div>
  );
}
