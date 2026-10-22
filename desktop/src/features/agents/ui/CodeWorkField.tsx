import type { EnvVarsValue } from "./EnvVarsEditor";
import { codeWorkEnabled, withCodeWork } from "./codeWorkSetting";
import { useT } from "@/shared/i18n";

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
  const t = useT();
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
        {t("agents.codeWork.label")}
      </label>
      <p className="text-xs text-muted-foreground" id={`${id}-description`}>
        {t("agents.codeWork.help")}
      </p>
    </div>
  );
}
