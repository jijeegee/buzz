import type {
  AcpRuntimeCatalogEntry,
  ManagedAgentBackend,
  RuntimeConfigSurface,
} from "@/shared/api/types";
import { PERSONA_LABEL_OPTIONAL_CLASS } from "./agentConfigOptions";
import {
  EFFORT_DEFAULT_LABEL,
  EFFORT_FALLBACK_HELPER_TEXT,
  effortPickerState,
  effortSelectionToPersistedValue,
} from "./effortPicker";
import { PersonaDropdownField } from "./PersonaDropdownField";

/**
 * Thinking-effort write control, shared by every model-selection surface
 * through `ModelEffortFields`.
 *
 * Local-only by construction: the Rust backend rejects effort writes for
 * non-local backends (remote effort is set at deploy time via `policy_env`). So
 * the control renders only for a local backend AND when it has a vocabulary to
 * offer: the running session's advertised `thought_level` option (`config`), or
 * — before any session, after a restart, or for a freshly picked runtime — the
 * prospective runtime's catalog `effortThoughtLevel.fallbackValues`
 * (`runtime`). Discovered options win; fallback-sourced options carry a "may
 * vary by model" hint. The read-only configured-vs-running two-facts display
 * lives in `AgentConfigPanel`; this is the write control.
 *
 * Save-gated, not direct-write: the control is fully controlled by the parent
 * dialog (`value`/`onChange`) and owns no mutation. The instance dialog persists
 * the selection by embedding `effortLevel` in the locked `update_managed_agent`
 * call (PR #4625); the definition dialog embeds it in `create_persona` /
 * `update_persona` as the definition-level default linked instances inherit.
 * So the effort write is atomic with the rest of the save and can never race
 * or survive a Cancel/failed Save.
 */
export type EffortPickerFieldProps = {
  backend: ManagedAgentBackend;
  /** Running-session surface, when one is valid for the prospective runtime. */
  config:
    | Pick<RuntimeConfigSurface, "effortConfigId" | "effortOptions">
    | undefined;
  /** Sentinel-row label; see `effortPickerState`'s `defaultLabel`. */
  defaultLabel?: string;
  disabled: boolean;
  id?: string;
  onChange: (level: string | null) => void;
  /** Prospective runtime's catalog entry — supplies the pre-discovery fallback. */
  runtime: Pick<AcpRuntimeCatalogEntry, "effortThoughtLevel"> | undefined;
  /** The pending persisted effort form (`null` = the sentinel). */
  value: string | null;
};

export function EffortPickerField({
  backend,
  config,
  defaultLabel,
  disabled,
  id = "edit-agent-effort",
  onChange,
  runtime,
  value,
}: EffortPickerFieldProps) {
  const { visible, options, selectValue, usingFallback } = effortPickerState({
    backend,
    effortConfigId: config?.effortConfigId,
    effortOptions: config?.effortOptions,
    fallbackValues: runtime?.effortThoughtLevel?.fallbackValues,
    currentEffort: value,
    defaultLabel,
  });

  if (!visible) {
    return null;
  }

  return (
    <div className="space-y-1.5">
      <label className="text-sm font-medium text-foreground" htmlFor={id}>
        Thinking effort
        <span className={PERSONA_LABEL_OPTIONAL_CLASS}>Optional</span>
      </label>
      <PersonaDropdownField
        disabled={disabled}
        id={id}
        onValueChange={(next) =>
          onChange(effortSelectionToPersistedValue(next))
        }
        options={options}
        placeholder={defaultLabel ?? EFFORT_DEFAULT_LABEL}
        value={selectValue}
      />
      <p className="text-xs text-muted-foreground">
        Applied at the next session start.
        {usingFallback ? ` ${EFFORT_FALLBACK_HELPER_TEXT}` : null}
      </p>
    </div>
  );
}
