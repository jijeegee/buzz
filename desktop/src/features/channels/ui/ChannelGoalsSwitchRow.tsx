import { PanelSectionGroup } from "@/shared/ui/PanelSectionGroup";
import { Switch } from "@/shared/ui/switch";

const SWITCH_ID = "channel-management-goals-switch";
const HINT_ID = `${SWITCH_ID}-hint`;

/**
 * Per-room goals switch in channel settings. Saves immediately (kind:9002
 * `["goals", "on" | "off"]`). People who may not change it see the current
 * state read-only: the switch stays visible but disabled, with a note.
 */
export function ChannelGoalsSwitchRow({
  canToggle,
  checked,
  error,
  isSaving,
  onCheckedChange,
}: {
  canToggle: boolean;
  checked: boolean;
  error: unknown;
  isSaving: boolean;
  onCheckedChange: (enabled: boolean) => void;
}) {
  return (
    <PanelSectionGroup testId="channel-management-goals">
      <div className="flex min-h-16 w-full items-center gap-3 px-4 py-3">
        <div className="min-w-0 flex-1">
          <label
            className="block text-sm font-medium text-foreground"
            htmlFor={SWITCH_ID}
          >
            Goals
          </label>
          <p className="mt-0.5 text-sm text-muted-foreground/70" id={HINT_ID}>
            Track goals for this conversation. Agents get goal guidance only
            when on.
            {canToggle ? null : " Only owners and admins can change this."}
          </p>
          {error instanceof Error ? (
            <p className="mt-1 text-sm text-destructive" role="alert">
              {error.message}
            </p>
          ) : null}
        </div>
        <Switch
          aria-describedby={HINT_ID}
          checked={checked}
          data-testid={SWITCH_ID}
          disabled={!canToggle || isSaving}
          id={SWITCH_ID}
          onCheckedChange={onCheckedChange}
        />
      </div>
    </PanelSectionGroup>
  );
}
