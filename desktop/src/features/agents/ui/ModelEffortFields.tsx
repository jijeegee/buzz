import type * as React from "react";

import {
  EffortPickerField,
  type EffortPickerFieldProps,
} from "./EffortPickerField";

/**
 * The effort inputs a model-selection surface hands to its companion picker;
 * `disabled` is owned by the wrapper so both controls share one save gate.
 */
export type ModelEffortCompanion = Omit<EffortPickerFieldProps, "disabled">;

/**
 * Model control + its thinking-effort companion, as one unit.
 *
 * Effort is a property of the model choice: whichever surface lets the user
 * pick a model (explicit model field, inherited-defaults summary, instance
 * provider/model block) renders its model control as `children` here and the
 * shared `EffortPickerField` follows directly beneath it. Centralizing the
 * pairing is what keeps a new model surface from shipping without effort.
 *
 * The companion still decides its own visibility through `effortPickerState`
 * (local backend AND a vocabulary: the running session's `thought_level`
 * option or the runtime catalog's `effortThoughtLevel.fallbackValues`), so a
 * harness without an ACP effort option (Goose, buzz-agent, custom) shows only
 * the model control. `effort: null` means the surface has no model selection
 * to pair with (nothing chosen yet) and renders the children alone.
 */
export function ModelEffortFields({
  children,
  disabled,
  effort,
}: {
  children: React.ReactNode;
  disabled: boolean;
  effort: ModelEffortCompanion | null;
}) {
  return (
    <div className="space-y-5" data-testid="model-effort-fields">
      {children}
      {effort ? <EffortPickerField disabled={disabled} {...effort} /> : null}
    </div>
  );
}
