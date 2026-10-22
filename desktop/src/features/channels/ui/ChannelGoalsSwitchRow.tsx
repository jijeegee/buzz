import { PanelSectionGroup } from "@/shared/ui/PanelSectionGroup";
import { Switch } from "@/shared/ui/switch";
import { useT } from "@/shared/i18n";

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
  const t = useT();
  return (
    <PanelSectionGroup testId="channel-management-goals">
      <div className="flex min-h-16 w-full items-center gap-3 px-4 py-3">
        <div className="min-w-0 flex-1">
          <label
            className="block text-sm font-medium text-foreground"
            htmlFor={SWITCH_ID}
          >
            {t("channels.goals.label")}
          </label>
          <p className="mt-0.5 text-sm text-muted-foreground/70" id={HINT_ID}>
            {canToggle
              ? t("channels.goals.hint")
              : t("channels.goals.hintReadOnly")}
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
