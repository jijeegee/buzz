import * as React from "react";

import {
  type AttachManagedAgentToChannelResult,
  useAddAgentsToChannelMutation,
  useAttachManagedAgentToChannelMutation,
} from "@/features/agents/hooks";
import {
  useChannelMembersQuery,
  useChannelsQuery,
} from "@/features/channels/hooks";
import type { Channel, ChannelRole, ManagedAgent } from "@/shared/api/types";
import { normalizePubkey } from "@/shared/lib/pubkey";
import { toUserFacingErrorDetail } from "@/shared/lib/userFacingError";
import { Button } from "@/shared/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui/dialog";
import { useT } from "@/shared/i18n";

/** An agent running on a paired computer: added as a channel member. */
export type ExternalChannelAgent = {
  kind: "external";
  pubkey: string;
  name: string;
};

function isExternal(
  agent: ManagedAgent | ExternalChannelAgent | null,
): agent is ExternalChannelAgent {
  return (agent as ExternalChannelAgent | null)?.kind === "external";
}

export function AddAgentToChannelDialog({
  agent,
  open,
  onAdded,
  onAddedExternal,
  onOpenChange,
}: {
  agent: ManagedAgent | ExternalChannelAgent | null;
  open: boolean;
  onAdded?: (
    channel: Channel,
    result: AttachManagedAgentToChannelResult,
  ) => void;
  /** After adding an external agent. */
  onAddedExternal?: (channel: Channel) => void;
  onOpenChange: (open: boolean) => void;
}) {
  const t = useT();
  const channelsQuery = useChannelsQuery();
  const [channelId, setChannelId] = React.useState("");
  const [role, setRole] = React.useState<Exclude<ChannelRole, "owner">>("bot");
  const attachMutation = useAttachManagedAgentToChannelMutation(
    channelId || null,
  );
  const addMutation = useAddAgentsToChannelMutation();
  const [addFailure, setAddFailure] = React.useState<string | null>(null);
  const attachAgentMutation = {
    isPending: attachMutation.isPending || addMutation.isPending,
    error:
      attachMutation.error ??
      addMutation.error ??
      (addFailure ? new Error(addFailure) : null),
  };
  const channels = React.useMemo(
    () =>
      (channelsQuery.data ?? []).filter(
        (channel) => channel.channelType !== "dm" && !channel.archivedAt,
      ),
    [channelsQuery.data],
  );

  function reset() {
    setChannelId("");
    setRole("bot");
    attachMutation.reset();
    addMutation.reset();
    setAddFailure(null);
  }

  function handleOpenChange(next: boolean) {
    if (!next) {
      reset();
    }

    onOpenChange(next);
  }

  React.useEffect(() => {
    if (!open) {
      return;
    }

    if (!channelId && channels.length > 0) {
      setChannelId(channels[0].id);
    }
  }, [channelId, channels, open]);

  const membersQuery = useChannelMembersQuery(
    channelId || null,
    open && !!channelId,
  );

  const isAlreadyMember = React.useMemo(() => {
    if (!agent?.pubkey || !membersQuery.data) {
      return false;
    }
    const normalized = normalizePubkey(agent.pubkey);
    return membersQuery.data.some(
      (member) => normalizePubkey(member.pubkey) === normalized,
    );
  }, [agent?.pubkey, membersQuery.data]);

  const selectedChannel =
    channels.find((channel) => channel.id === channelId) ?? null;

  async function handleSubmit() {
    if (!agent || !selectedChannel) {
      return;
    }

    try {
      if (isExternal(agent)) {
        setAddFailure(null);
        const result = await addMutation.mutateAsync({
          channelId: selectedChannel.id,
          pubkeys: [agent.pubkey],
          role,
        });
        const failure = result.failures[0];
        if (failure) {
          setAddFailure(failure.error);
          return;
        }
        onAddedExternal?.(selectedChannel);
        handleOpenChange(false);
        return;
      }
      const result = await attachMutation.mutateAsync({
        agent,
        role,
      });

      onAdded?.(selectedChannel, result);
      handleOpenChange(false);
    } catch {
      // React Query stores the error; keep the dialog open and render it inline.
    }
  }

  return (
    <Dialog onOpenChange={handleOpenChange} open={open}>
      <DialogContent className="max-w-xl overflow-hidden p-0">
        <div className="flex max-h-[85vh] flex-col">
          <DialogHeader className="shrink-0 border-b border-border/60 px-6 py-5 pr-14">
            <DialogTitle>{t("agents.addToGroup.title")}</DialogTitle>
            <DialogDescription>
              {agent?.name
                ? t("agents.addToGroup.description", { name: agent.name })
                : t("agents.addToGroup.descriptionUnnamed")}
            </DialogDescription>
          </DialogHeader>

          <div className="min-h-0 flex-1 space-y-5 overflow-y-auto px-6 py-5">
            <div className="space-y-1.5">
              <label className="text-sm font-medium" htmlFor="agent-channel-id">
                {t("agents.addToGroup.group")}
              </label>
              <select
                className="flex h-9 w-full rounded-md border border-input bg-background px-3 py-2 text-sm shadow-xs"
                disabled={
                  channels.length === 0 || attachAgentMutation.isPending
                }
                id="agent-channel-id"
                onChange={(event) => setChannelId(event.target.value)}
                value={channelId}
              >
                {channels.length === 0 ? (
                  <option value="">{t("agents.addToGroup.noGroups")}</option>
                ) : null}
                {channels.map((channel) => (
                  <option key={channel.id} value={channel.id}>
                    {channel.name} · {channel.visibility}
                  </option>
                ))}
              </select>
              <p className="text-xs text-muted-foreground">
                {t("agents.addToGroup.groupHelp")}
              </p>
            </div>

            {isAlreadyMember ? (
              <div className="flex items-center gap-2 rounded-lg border border-border/50 bg-muted/30 px-3 py-2 text-sm text-muted-foreground">
                <span>✓</span>
                <span>{t("agents.addToGroup.alreadyMember")}</span>
              </div>
            ) : null}

            <div className="space-y-1.5">
              <label
                className="text-sm font-medium"
                htmlFor="agent-channel-role"
              >
                {t("agents.addToGroup.role")}
              </label>
              <select
                className="flex h-9 w-full rounded-md border border-input bg-background px-3 py-2 text-sm shadow-xs"
                disabled={attachAgentMutation.isPending}
                id="agent-channel-role"
                onChange={(event) =>
                  setRole(event.target.value as Exclude<ChannelRole, "owner">)
                }
                value={role}
              >
                <option value="bot">{t("agents.addToGroup.roleBot")}</option>
                <option value="member">
                  {t("agents.addToGroup.roleMember")}
                </option>
                <option value="guest">
                  {t("agents.addToGroup.roleGuest")}
                </option>
                <option value="admin">
                  {t("agents.addToGroup.roleAdmin")}
                </option>
              </select>
            </div>

            {channelsQuery.error instanceof Error ? (
              <p className="rounded-2xl border border-destructive/30 bg-destructive/10 px-4 py-3 text-sm text-destructive">
                {toUserFacingErrorDetail(channelsQuery.error.message)}
              </p>
            ) : null}

            {attachAgentMutation.error instanceof Error ? (
              <p className="rounded-2xl border border-destructive/30 bg-destructive/10 px-4 py-3 text-sm text-destructive">
                {toUserFacingErrorDetail(attachAgentMutation.error.message)}
              </p>
            ) : null}
          </div>

          <div className="flex shrink-0 justify-end gap-2 border-t border-border/60 px-6 py-4">
            <Button
              onClick={() => handleOpenChange(false)}
              size="sm"
              type="button"
              variant="outline"
            >
              {t("common.cancel")}
            </Button>
            <Button
              disabled={
                !agent ||
                !selectedChannel ||
                channelsQuery.isLoading ||
                attachAgentMutation.isPending
              }
              onClick={() => void handleSubmit()}
              size="sm"
              type="button"
            >
              {attachAgentMutation.isPending
                ? t("agents.addToGroup.adding")
                : isAlreadyMember
                  ? t("agents.addToGroup.readd")
                  : t("agents.addToGroup.add")}
            </Button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
