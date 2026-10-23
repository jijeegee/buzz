import { LeaveChannelAlertDialog } from "@/features/sidebar/ui/ChannelSectionDialogs";

import { useOwnAgentsToRemoveOnLeave } from "./useOwnAgentsToRemoveOnLeave";

/**
 * Leave confirmation for the room settings panel. When I have agents in the
 * room it offers to remove them first; `onLeave` gets the agents to remove
 * (empty for "Leave only" or when there are none).
 */
export function LeaveChannelConfirmDialog({
  channelId,
  channelName,
  isChat = false,
  onLeave,
  onOpenChange,
  open,
}: {
  channelId: string;
  channelName: string;
  isChat?: boolean;
  onLeave: (removeAgentPubkeys: readonly string[]) => void;
  onOpenChange: (open: boolean) => void;
  open: boolean;
}) {
  // Mounted only inside the open settings panel, whose roster is loaded.
  const ownAgents = useOwnAgentsToRemoveOnLeave(channelId, true);
  return (
    <LeaveChannelAlertDialog
      channelName={channelName}
      isChat={isChat}
      onConfirm={(removeOwnAgents) => {
        onOpenChange(false);
        onLeave(removeOwnAgents ? ownAgents : []);
      }}
      onOpenChange={onOpenChange}
      open={open}
      ownAgentCount={ownAgents.length}
    />
  );
}
