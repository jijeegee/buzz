import type { EnvVarsValue } from "./EnvVarsEditor";
import { codeWorkEnabled, withCodeWork } from "./codeWorkSetting";

/** The per-agent "Code work" checkbox; see `codeWorkSetting`. */
export function CodeWorkField({
  disabled,
  envVars,
  id,
  inheritedEnvVars,
  onEnvVarsChange,
}: {
  disabled: boolean;
  envVars: EnvVarsValue;
  id: string;
  inheritedEnvVars?: EnvVarsValue;
  onEnvVarsChange: (value: EnvVarsValue) => void;
}) {
  return (
    <div className="space-y-1.5">
      <label
        className="flex items-center gap-2 text-sm font-medium"
        htmlFor={id}
      >
        <input
          aria-describedby={`${id}-description`}
          checked={codeWorkEnabled(envVars, inheritedEnvVars)}
          disabled={disabled}
          id={id}
          onChange={(event) =>
            onEnvVarsChange(
              withCodeWork(envVars, event.target.checked, inheritedEnvVars),
            )
          }
          type="checkbox"
        />
        Code work
      </label>
      <p className="text-xs text-muted-foreground" id={`${id}-description`}>
        Include engineering and repository guidance in this agent's
        instructions.
      </p>
    </div>
  );
}
