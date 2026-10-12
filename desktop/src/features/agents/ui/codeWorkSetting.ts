import type { EnvVarsValue } from "./EnvVarsEditor";

/**
 * The per-agent "Code work" setting lives in the agent's env vars: buzz-acp
 * leaves the engineering and repository guidance out of its base prompt when
 * this is `"true"`. Absent means on, so existing agents keep that guidance.
 * buzz-acp accepts exactly `"true"` or `"false"`.
 */
export const CODE_WORK_OFF_ENV_KEY = "BUZZ_ACP_NO_CODE_WORK";

/** Whether Code work is on, reading own env over the inherited layer. */
export function codeWorkEnabled(
  envVars: EnvVarsValue,
  inheritedEnvVars: EnvVarsValue = {},
): boolean {
  const value =
    envVars[CODE_WORK_OFF_ENV_KEY] ?? inheritedEnvVars[CODE_WORK_OFF_ENV_KEY];
  return value !== "true";
}

/**
 * Env vars with Code work set to `enabled`. The key is dropped when the
 * inherited layer already gives that value, so the default stays implicit.
 */
export function withCodeWork(
  envVars: EnvVarsValue,
  enabled: boolean,
  inheritedEnvVars: EnvVarsValue = {},
): EnvVarsValue {
  const next = { ...envVars };
  delete next[CODE_WORK_OFF_ENV_KEY];
  if (codeWorkEnabled({}, inheritedEnvVars) !== enabled) {
    next[CODE_WORK_OFF_ENV_KEY] = enabled ? "false" : "true";
  }
  return next;
}
