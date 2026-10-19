import { cn } from "@/shared/lib/cn";
import { PersonaDropdownField } from "./PersonaDropdownField";
import {
  PERSONA_FIELD_CONTROL_CLASS,
  PERSONA_FIELD_SHELL_CLASS,
} from "./agentConfigOptions";
import type { HarnessProviderChoice } from "./usePersonaModelDiscovery";

/**
 * The provider of a harness that owns its providers (Hermes). The user set
 * them up in the harness, so Kuzz only shows them: one is read-only, two or
 * more become a choice. Picking one sets the model (unset for the default).
 */
export function HarnessProviderField({
  choice,
  disabled,
  harnessLabel,
  id,
  note,
  onModelChange,
}: {
  choice: HarnessProviderChoice;
  disabled?: boolean;
  harnessLabel: string;
  id: string;
  /** Extra line under the field, e.g. where the list comes from. */
  note?: string;
  onModelChange: (model: string) => void;
}) {
  const { providers, selectedId } = choice;
  const only = providers.length === 1 ? providers[0] : undefined;
  const hint = `Set up in ${harnessLabel}. Change it there.`;

  return (
    <div className="space-y-1.5">
      <label className="text-sm font-medium text-foreground" htmlFor={id}>
        LLM provider
      </label>
      {only ? (
        <div
          className={cn(
            "flex min-h-11 items-center px-3",
            PERSONA_FIELD_SHELL_CLASS,
          )}
        >
          <span
            className={cn("text-sm leading-6", PERSONA_FIELD_CONTROL_CLASS)}
            id={id}
          >
            {only.label}
          </span>
        </div>
      ) : (
        <PersonaDropdownField
          disabled={disabled}
          id={id}
          onValueChange={(providerId) =>
            onModelChange(choice.modelFor(providerId))
          }
          options={providers.map((provider) => ({
            label: provider.isDefault
              ? `${provider.label} (${harnessLabel} default)`
              : provider.label,
            value: provider.id,
          }))}
          placeholder="Choose a provider"
          value={selectedId ?? ""}
        />
      )}
      <p className="text-xs text-muted-foreground">
        {note ? `${hint} ${note}` : hint}
      </p>
    </div>
  );
}
