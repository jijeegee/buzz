import { NO_DEFAULT_AI_HINT } from "@/features/agents/lib/defaultAi";
import type { ManagedAgent } from "@/shared/api/types";
import { cn } from "@/shared/lib/cn";
import { Switch } from "@/shared/ui/switch";

/**
 * The "Add your default AI" switch row shared by the create-channel and
 * create-project forms. Without a starred agent the row stays visible but
 * disabled and explains where to star one, so the Agents setting
 * ("Add default AI to new channels") never looks like it silently did
 * nothing. `resolveAddDefaultAi` still forces the payload to `false` then.
 */
export function AddDefaultAiRow({
  checked,
  defaultAi,
  disabled,
  idPrefix,
  joinTarget,
  onCheckedChange,
}: {
  checked: boolean;
  defaultAi: Pick<ManagedAgent, "name"> | null;
  disabled: boolean;
  /** `create-channel` or `create-project`; keeps each form's ids and test ids. */
  idPrefix: string;
  /** Where the agent lands, e.g. "this channel" or "the project home". */
  joinTarget: string;
  onCheckedChange: (value: boolean) => void;
}) {
  const switchId = `${idPrefix}-add-default-ai`;
  const hasDefaultAi = defaultAi !== null;

  return (
    <div
      className="flex min-h-12 items-center justify-between gap-4 rounded-xl border border-input bg-background px-3 py-3"
      data-default-ai={hasDefaultAi ? "set" : "missing"}
      data-testid={`${idPrefix}-default-ai-container`}
    >
      <div
        className={cn("min-w-0", (disabled || !hasDefaultAi) && "opacity-50")}
      >
        <label
          className="text-sm font-medium text-foreground"
          htmlFor={switchId}
        >
          Add your default AI
        </label>
        <p className="text-xs text-muted-foreground">
          {defaultAi
            ? `${defaultAi.name} joins ${joinTarget} as a bot`
            : NO_DEFAULT_AI_HINT}
        </p>
      </div>
      <Switch
        checked={hasDefaultAi && checked}
        data-testid={switchId}
        disabled={disabled || !hasDefaultAi}
        id={switchId}
        onCheckedChange={onCheckedChange}
      />
    </div>
  );
}
