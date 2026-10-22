import * as React from "react";

import { useAddAgentsToChannelMutation } from "@/features/agents/hooks";
import {
  MY_TEAM_MAX_AGENTS,
  MY_TEAM_MAX_DESCRIPTION_CHARS,
  type MyTeam,
} from "@/features/agents/lib/myTeams";
import { useChannelsQuery } from "@/features/channels/hooks";
import type { ChannelRole } from "@/shared/api/types";
import { toUserFacingErrorDetail } from "@/shared/lib/userFacingError";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/shared/ui/alert-dialog";
import { Button } from "@/shared/ui/button";
import { Checkbox } from "@/shared/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui/dialog";
import { Input } from "@/shared/ui/input";
import { Textarea } from "@/shared/ui/textarea";
import type { TeamAgent } from "./MyTeamsSection";

export type MyTeamDraft = {
  name: string;
  description: string;
  agentPubkeys: string[];
};

/**
 * Make or edit a team: a name, an optional description and which of my
 * agents are in it, from any computer (grouped by computer).
 */
export function MyTeamDialog({
  agents,
  isSaving,
  open,
  team,
  onOpenChange,
  onSave,
}: {
  /** Every real agent of the owner, this computer's first. */
  agents: readonly TeamAgent[];
  isSaving: boolean;
  open: boolean;
  /** The team being edited; `null` for a new team. */
  team: MyTeam | null;
  onOpenChange: (open: boolean) => void;
  onSave: (draft: MyTeamDraft) => Promise<void>;
}) {
  const [name, setName] = React.useState("");
  const [description, setDescription] = React.useState("");
  const [selected, setSelected] = React.useState<string[]>([]);
  const [error, setError] = React.useState<string | null>(null);

  React.useEffect(() => {
    if (!open) return;
    setName(team?.name ?? "");
    setDescription(team?.description ?? "");
    setSelected(team?.agentPubkeys ?? []);
    setError(null);
  }, [open, team]);

  const groups = React.useMemo(() => {
    const byComputer = new Map<string, TeamAgent[]>();
    for (const agent of agents) {
      const list = byComputer.get(agent.computer) ?? [];
      list.push(agent);
      byComputer.set(agent.computer, list);
    }
    return [...byComputer.entries()];
  }, [agents]);
  const known = new Set(agents.map((agent) => agent.pubkey));
  // Members that are no longer the owner's agents stay unless unticked.
  const removed = selected.filter((pubkey) => !known.has(pubkey));

  function toggle(pubkey: string, checked: boolean) {
    setSelected((current) =>
      checked
        ? current.includes(pubkey)
          ? current
          : [...current, pubkey]
        : current.filter((other) => other !== pubkey),
    );
  }

  async function submit(event: React.FormEvent) {
    event.preventDefault();
    if (!name.trim()) {
      setError("Give the team a name.");
      return;
    }
    if (selected.length > MY_TEAM_MAX_AGENTS) {
      setError(`A team can have up to ${MY_TEAM_MAX_AGENTS} agents.`);
      return;
    }
    try {
      await onSave({ name, description, agentPubkeys: selected });
      onOpenChange(false);
    } catch (saveError) {
      setError(
        saveError instanceof Error
          ? toUserFacingErrorDetail(saveError.message)
          : "Could not save the team.",
      );
    }
  }

  return (
    <Dialog onOpenChange={onOpenChange} open={open}>
      <DialogContent className="max-w-xl overflow-hidden p-0">
        <form
          className="flex max-h-[85vh] flex-col"
          data-testid="my-team-dialog"
          onSubmit={(event) => void submit(event)}
        >
          <DialogHeader className="shrink-0 border-b border-border/60 px-6 py-5 pr-14">
            <DialogTitle>{team ? "Edit team" : "New team"}</DialogTitle>
            <DialogDescription>
              A team is a group of your agents, on any of your computers. Adding
              it to a group adds these agents.
            </DialogDescription>
          </DialogHeader>
          <div className="min-h-0 flex-1 space-y-5 overflow-y-auto px-6 py-5">
            <div className="space-y-1.5">
              <label className="text-sm font-medium" htmlFor="my-team-name">
                Name
              </label>
              <Input
                autoFocus
                data-testid="my-team-name"
                id="my-team-name"
                maxLength={200}
                onChange={(event) => setName(event.target.value)}
                value={name}
              />
            </div>
            <div className="space-y-1.5">
              <label
                className="text-sm font-medium"
                htmlFor="my-team-description"
              >
                Description
              </label>
              <Textarea
                id="my-team-description"
                maxLength={MY_TEAM_MAX_DESCRIPTION_CHARS}
                onChange={(event) => setDescription(event.target.value)}
                rows={2}
                value={description}
              />
            </div>
            <fieldset className="space-y-3">
              <legend className="text-sm font-medium">Agents</legend>
              {agents.length === 0 ? (
                <p className="text-sm text-muted-foreground">
                  You have no agents yet.
                </p>
              ) : null}
              {groups.map(([computer, list]) => (
                <div className="space-y-1" key={computer}>
                  <p className="text-xs font-semibold text-muted-foreground">
                    {computer}
                  </p>
                  {list.map((agent) => (
                    <div
                      className="flex items-center gap-2 rounded-md px-1 py-1 text-sm hover:bg-muted/60"
                      key={agent.pubkey}
                    >
                      <Checkbox
                        checked={selected.includes(agent.pubkey)}
                        data-testid={`my-team-agent-${agent.pubkey}`}
                        id={`my-team-agent-${agent.pubkey}`}
                        onCheckedChange={(checked) =>
                          toggle(agent.pubkey, checked === true)
                        }
                      />
                      <label
                        className="min-w-0 flex-1 truncate"
                        htmlFor={`my-team-agent-${agent.pubkey}`}
                      >
                        {agent.name}
                      </label>
                    </div>
                  ))}
                </div>
              ))}
              {removed.length > 0 ? (
                <p className="text-xs text-muted-foreground">
                  {removed.length === 1
                    ? "1 agent in this team was not found on your computers (removed, or its computer has not checked in); saving keeps it."
                    : `${removed.length} agents in this team were not found on your computers (removed, or their computer has not checked in); saving keeps them.`}
                </p>
              ) : null}
            </fieldset>
            {error ? (
              <p className="rounded-2xl border border-destructive/30 bg-destructive/10 px-4 py-3 text-sm text-destructive">
                {error}
              </p>
            ) : null}
          </div>
          <div className="flex shrink-0 justify-end gap-2 border-t border-border/60 px-6 py-4">
            <Button
              onClick={() => onOpenChange(false)}
              size="sm"
              type="button"
              variant="outline"
            >
              Cancel
            </Button>
            <Button
              data-testid="my-team-save"
              disabled={isSaving}
              size="sm"
              type="submit"
            >
              {isSaving ? "Saving…" : team ? "Save" : "Create team"}
            </Button>
          </div>
        </form>
      </DialogContent>
    </Dialog>
  );
}

/**
 * Add a team's agents to a channel. Nothing is created: the agents that are
 * already members stay, the others are added (and started, on this
 * computer).
 */
export function AddMyTeamToChannelDialog({
  agentsByPubkey,
  team,
  onAdded,
  onOpenChange,
}: {
  agentsByPubkey: ReadonlyMap<string, TeamAgent>;
  team: MyTeam | null;
  onAdded: (message: string) => void;
  onOpenChange: (open: boolean) => void;
}) {
  const open = team !== null;
  const channelsQuery = useChannelsQuery();
  const mutation = useAddAgentsToChannelMutation();
  const [channelId, setChannelId] = React.useState("");
  const [role, setRole] = React.useState<Exclude<ChannelRole, "owner">>("bot");
  const channels = React.useMemo(
    () =>
      (channelsQuery.data ?? []).filter(
        (channel) => channel.channelType !== "dm" && !channel.archivedAt,
      ),
    [channelsQuery.data],
  );
  const members = (team?.agentPubkeys ?? []).filter((pubkey) =>
    agentsByPubkey.has(pubkey),
  );
  // Not on any of the owner's computers now: never added to a channel.
  const notFound = (team?.agentPubkeys.length ?? 0) - members.length;

  React.useEffect(() => {
    if (!open) {
      setChannelId("");
      setRole("bot");
      mutation.reset();
      return;
    }
    if (!channelId && channels.length > 0) setChannelId(channels[0].id);
  }, [channelId, channels, mutation.reset, open]);

  const channel = channels.find((other) => other.id === channelId) ?? null;
  const nameOf = (pubkey: string) => agentsByPubkey.get(pubkey)?.name ?? pubkey;

  async function submit() {
    if (!team || !channel) return;
    try {
      const result = await mutation.mutateAsync({
        channelId: channel.id,
        pubkeys: members,
        role,
      });
      if (result.failures.length > 0) {
        const added = result.added.map(nameOf);
        throw new Error(
          [
            ...(added.length > 0 ? [`Added: ${added.join(", ")}.`] : []),
            ...result.failures.map(
              (failure) => `${nameOf(failure.pubkey)}: ${failure.error}`,
            ),
          ].join("\n"),
        );
      }
      const added = result.added.length;
      onAdded(
        added === 0
          ? `${team.name} is already in ${channel.name}.`
          : `Added ${added === 1 ? "1 agent" : `${added} agents`} of ${team.name} to ${channel.name}.`,
      );
      onOpenChange(false);
    } catch {
      // The mutation keeps the error; it is shown below.
    }
  }

  const failure =
    mutation.error instanceof Error
      ? toUserFacingErrorDetail(mutation.error.message)
      : null;

  return (
    <Dialog onOpenChange={onOpenChange} open={open}>
      <DialogContent
        className="max-w-xl overflow-hidden p-0"
        data-testid="add-my-team-to-channel-dialog"
      >
        <div className="flex max-h-[85vh] flex-col">
          <DialogHeader className="shrink-0 border-b border-border/60 px-6 py-5 pr-14">
            <DialogTitle>Add {team?.name ?? "team"} to a group</DialogTitle>
            <DialogDescription>
              Adds these agents to the group. No new agents are made.
            </DialogDescription>
          </DialogHeader>
          <div className="min-h-0 flex-1 space-y-5 overflow-y-auto px-6 py-5">
            <p className="text-sm">
              {members.map(nameOf).join(", ") || "This team has no agents."}
            </p>
            {notFound > 0 ? (
              <p
                className="text-xs text-muted-foreground"
                data-testid="add-my-team-not-found"
              >
                {notFound === 1
                  ? "1 agent is not added: it was not found on your computers."
                  : `${notFound} agents are not added: they were not found on your computers.`}
              </p>
            ) : null}
            <div className="space-y-1.5">
              <label
                className="text-sm font-medium"
                htmlFor="my-team-channel-id"
              >
                Group
              </label>
              <select
                className="flex h-9 w-full rounded-md border border-input bg-background px-3 py-2 text-sm shadow-xs"
                disabled={channels.length === 0 || mutation.isPending}
                id="my-team-channel-id"
                onChange={(event) => setChannelId(event.target.value)}
                value={channelId}
              >
                {channels.length === 0 ? (
                  <option value="">No groups available</option>
                ) : null}
                {channels.map((other) => (
                  <option key={other.id} value={other.id}>
                    {other.name} · {other.visibility}
                  </option>
                ))}
              </select>
            </div>
            <div className="space-y-1.5">
              <label className="text-sm font-medium" htmlFor="my-team-role">
                Role
              </label>
              <select
                className="flex h-9 w-full rounded-md border border-input bg-background px-3 py-2 text-sm shadow-xs"
                disabled={mutation.isPending}
                id="my-team-role"
                onChange={(event) =>
                  setRole(event.target.value as Exclude<ChannelRole, "owner">)
                }
                value={role}
              >
                <option value="bot">bot</option>
                <option value="member">member</option>
                <option value="guest">guest</option>
                <option value="admin">admin</option>
              </select>
            </div>
            {failure ? (
              <p className="whitespace-pre-line rounded-2xl border border-destructive/30 bg-destructive/10 px-4 py-3 text-sm text-destructive">
                {failure}
              </p>
            ) : null}
          </div>
          <div className="flex shrink-0 justify-end gap-2 border-t border-border/60 px-6 py-4">
            <Button
              onClick={() => onOpenChange(false)}
              size="sm"
              type="button"
              variant="outline"
            >
              Cancel
            </Button>
            <Button
              data-testid="add-my-team-to-channel-submit"
              disabled={!channel || members.length === 0 || mutation.isPending}
              onClick={() => void submit()}
              size="sm"
              type="button"
            >
              {mutation.isPending ? "Adding…" : "Add to group"}
            </Button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}

/** Confirm deleting a team. Its agents keep running and stay in channels. */
export function MyTeamDeleteDialog({
  team,
  onConfirm,
  onOpenChange,
}: {
  team: MyTeam | null;
  onConfirm: (team: MyTeam) => void;
  onOpenChange: (open: boolean) => void;
}) {
  return (
    <AlertDialog onOpenChange={onOpenChange} open={team !== null}>
      <AlertDialogContent data-testid="my-team-delete-dialog">
        <AlertDialogHeader>
          <AlertDialogTitle>Delete team?</AlertDialogTitle>
          <AlertDialogDescription>
            {team
              ? `Delete "${team.name}". Its agents keep running and stay in their groups.`
              : "Delete this team."}
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel asChild>
            <Button type="button" variant="outline">
              Cancel
            </Button>
          </AlertDialogCancel>
          <AlertDialogAction asChild>
            <Button
              data-testid="my-team-delete-confirm"
              onClick={() => {
                if (team) onConfirm(team);
              }}
              type="button"
              variant="destructive"
            >
              Delete
            </Button>
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
