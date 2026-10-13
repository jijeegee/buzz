import * as React from "react";

import { useUpdateChannelMutation } from "@/features/channels/hooks";
import type { Channel } from "@/shared/api/types";

/**
 * State and save path for the per-room goals switch in channel settings.
 *
 * Who may flip it: any participant in a DM; elsewhere the same owner/admin
 * rule as name and description edits (`canManageChannel`). The switch saves
 * immediately as a kind:9002 edit carrying only `["goals", "on" | "off"]`.
 * It uses its own mutation instance so a failure stays on the switch row and
 * never shows up in (or blocks) the edit dialog.
 */
export function useChannelGoalsSwitch(
  channel: Pick<Channel, "channelType" | "goalsEnabled" | "id"> | null,
  canManageChannel: boolean,
) {
  const mutation = useUpdateChannelMutation(channel?.id ?? null);
  const canToggle = channel?.channelType === "dm" ? true : canManageChannel;
  const pendingEnabled = mutation.isPending
    ? mutation.variables?.goalsEnabled
    : undefined;
  // Show the requested state while the edit is in flight.
  const enabled = pendingEnabled ?? channel?.goalsEnabled === true;
  const { mutate } = mutation;
  const setEnabled = React.useCallback(
    (next: boolean) => {
      // Failures render on the switch row via `error`.
      mutate({ goalsEnabled: next });
    },
    [mutate],
  );

  return {
    canToggle,
    enabled,
    error: mutation.error,
    isSaving: mutation.isPending,
    setEnabled,
  };
}
