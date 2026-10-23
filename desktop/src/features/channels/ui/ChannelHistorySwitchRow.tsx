import { useUpdateChannelMutation } from "@/features/channels/hooks";
import {
  canToggleChannelHistory,
  historyForSwitch,
  newMembersSeeHistory,
} from "@/features/channels/lib/channelHistory";
import type { Channel } from "@/shared/api/types";
import { useT } from "@/shared/i18n";
import { PanelSectionGroup } from "@/shared/ui/PanelSectionGroup";
import { Switch } from "@/shared/ui/switch";

const SWITCH_ID = "channel-management-history-switch";
const HINT_ID = `${SWITCH_ID}-hint`;

/**
 * Group setting: whether people who join later can read what was said before.
 * Owners and admins only; hidden for everyone else, DMs and group chats.
 * Saves immediately as a kind:9002 edit carrying only `["history", value]`,
 * on its own mutation so a failure stays on this row.
 */
export function ChannelHistorySwitchRow({
  canManageChannel,
  channel,
}: {
  canManageChannel: boolean;
  channel: Pick<Channel, "channelType" | "groupChat" | "history" | "id">;
}) {
  const t = useT();
  const mutation = useUpdateChannelMutation(channel.id);
  if (!canToggleChannelHistory(channel, canManageChannel)) return null;

  // Show the requested state while the edit is in flight.
  const pending = mutation.isPending ? mutation.variables?.history : undefined;
  const checked = pending ? pending === "all" : newMembersSeeHistory(channel);

  return (
    <PanelSectionGroup testId="channel-management-history">
      <div className="flex min-h-16 w-full items-center gap-3 px-4 py-3">
        <div className="min-w-0 flex-1">
          <label
            className="block text-sm font-medium text-foreground"
            htmlFor={SWITCH_ID}
          >
            {t("channels.settings.historyLabel")}
          </label>
          <p className="mt-0.5 text-sm text-muted-foreground/70" id={HINT_ID}>
            {t("channels.settings.historyHint")}
          </p>
          {mutation.error instanceof Error ? (
            <p className="mt-1 text-sm text-destructive" role="alert">
              {mutation.error.message}
            </p>
          ) : null}
        </div>
        <Switch
          aria-describedby={HINT_ID}
          checked={checked}
          data-testid={SWITCH_ID}
          disabled={mutation.isPending}
          id={SWITCH_ID}
          onCheckedChange={(next) => {
            mutation.mutate({ history: historyForSwitch(next) });
          }}
        />
      </div>
    </PanelSectionGroup>
  );
}
