import { useQuery } from "@tanstack/react-query";
import { ChevronRight } from "lucide-react";
import * as React from "react";

import { ModelEffortFields } from "@/features/agents/ui/ModelEffortFields";
import { useGlobalAgentConfig } from "@/features/agents/useGlobalAgentConfig";
import { discoverAgentModels } from "@/shared/api/agentModels";
import type {
  TaskModelProviderOption,
  TaskModelStatus,
} from "@/shared/api/tauriMessageRouting";
import type { TaskModelTask } from "../../lib/taskModels";
import { useSetTaskModelMutation } from "../../taskModelsHooks";

const SELECT_CLASS =
  "flex h-9 w-full min-w-0 rounded-md border border-input bg-background px-3 py-2 text-sm shadow-xs disabled:cursor-not-allowed disabled:opacity-60";

/** Sentinel for the "automatic" choice in a native select. */
const AUTOMATIC = "";

/**
 * Models the provider lists for this key, for the Model select. A failed or
 * slow discovery only leaves the Default row and the saved model.
 */
function useProviderModels(
  provider: TaskModelProviderOption | null,
  apiKey: string | null,
) {
  return useQuery({
    enabled: provider?.hasKey === true,
    queryKey: ["task-model-discovery", provider?.id ?? "", apiKey ?? ""],
    queryFn: async () => {
      if (!provider) return [];
      const keyEnv = PROVIDER_KEY_ENV[provider.id];
      const response = await discoverAgentModels({
        agentCommand: "buzz-agent",
        provider: provider.id,
        envVars: keyEnv && apiKey ? { [keyEnv]: apiKey } : {},
      });
      return response.models.map((model) => model.id);
    },
    retry: false,
    staleTime: 5 * 60_000,
  });
}

/** Mirrors Rust `RouterProvider::key_env`; only used to feed discovery. */
const PROVIDER_KEY_ENV: Record<string, string> = {
  anthropic: "ANTHROPIC_API_KEY",
  openai: "OPENAI_COMPAT_API_KEY",
  openrouter: "OPENROUTER_API_KEY",
};

/**
 * One Settings › Models › Task models row: provider and model for an app
 * task (no effort — the router never thinks, so `ModelEffortFields` gets
 * `effort: null`). Each change is one `set_task_model`; a failure shows
 * inline and the saved state stays what Rust reports.
 */
export function TaskModelRow({
  onOpenProviders,
  status,
  task,
}: {
  onOpenProviders: () => void;
  status: TaskModelStatus;
  task: TaskModelTask;
}) {
  const providerId = React.useId();
  const modelId = React.useId();
  const statusId = React.useId();
  const { globalConfig } = useGlobalAgentConfig();
  const { mutate: save, ...saveMutation } = useSetTaskModelMutation();

  const savedProvider = status.provider ?? AUTOMATIC;
  const savedModel = status.model ?? AUTOMATIC;
  const effectiveProvider =
    status.providers.find(
      (provider) =>
        provider.id === (status.provider ?? status.effectiveProvider),
    ) ?? null;
  const discovery = useProviderModels(
    effectiveProvider,
    effectiveProvider
      ? (globalConfig.env_vars[PROVIDER_KEY_ENV[effectiveProvider.id]] ?? null)
      : null,
  );
  const modelOptions = React.useMemo(() => {
    const ids = new Set(discovery.data ?? []);
    if (status.model) ids.add(status.model);
    if (effectiveProvider) ids.delete(effectiveProvider.defaultModel);
    return [...ids].sort((a, b) => a.localeCompare(b));
  }, [discovery.data, effectiveProvider, status.model]);

  const write = (provider: string, model: string) =>
    save({
      taskId: task.id,
      provider: provider === AUTOMATIC ? null : provider,
      model: model === AUTOMATIC ? null : model,
    });

  return (
    <div
      className="space-y-3 px-4 py-3 text-sm"
      data-testid={`settings-models-task-${task.id}`}
    >
      <div className="min-w-0">
        <p className="font-medium text-foreground">{task.label}</p>
        <p
          className="mt-0.5 text-sm text-muted-foreground/70"
          data-settings-subcopy
        >
          {task.description}
        </p>
        <p
          className="mt-1 text-xs text-muted-foreground/70"
          data-settings-subcopy
        >
          The messages you write in channels — including drafts while you type —
          plus your agents' names and descriptions are sent to this provider to
          pick an agent.
        </p>
      </div>
      <ModelEffortFields disabled={saveMutation.isPending} effort={null}>
        <div className="grid gap-3 sm:grid-cols-2">
          <div className="space-y-1">
            <label
              className="text-xs font-medium text-foreground"
              htmlFor={providerId}
            >
              Provider
            </label>
            <select
              aria-describedby={statusId}
              className={SELECT_CLASS}
              data-testid={`settings-models-task-${task.id}-provider`}
              disabled={saveMutation.isPending}
              id={providerId}
              onChange={(event) => write(event.target.value, AUTOMATIC)}
              value={savedProvider}
            >
              <option value={AUTOMATIC}>Automatic</option>
              {status.providers.map((provider) => (
                <option
                  disabled={!provider.hasKey && provider.id !== savedProvider}
                  key={provider.id}
                  value={provider.id}
                >
                  {provider.hasKey
                    ? provider.label
                    : `${provider.label} — add an API key in Providers`}
                </option>
              ))}
            </select>
          </div>
          <div className="space-y-1">
            <label
              className="text-xs font-medium text-foreground"
              htmlFor={modelId}
            >
              Model
            </label>
            <select
              className={SELECT_CLASS}
              data-testid={`settings-models-task-${task.id}-model`}
              disabled={saveMutation.isPending || effectiveProvider === null}
              id={modelId}
              onChange={(event) => write(savedProvider, event.target.value)}
              value={savedModel}
            >
              <option value={AUTOMATIC}>
                {effectiveProvider
                  ? `Default — ${effectiveProvider.defaultModel} (cheapest)`
                  : "Default"}
              </option>
              {modelOptions.map((model) => (
                <option key={model} value={model}>
                  {model}
                </option>
              ))}
            </select>
          </div>
        </div>
      </ModelEffortFields>
      <div
        className="flex flex-wrap items-center gap-x-2 text-xs"
        data-testid={`settings-models-task-${task.id}-status`}
        id={statusId}
      >
        {status.ready ? (
          <span className="text-muted-foreground">
            Ready — {status.modelLabel ?? status.effectiveModel}
            {effectiveProvider ? ` (${effectiveProvider.label})` : null}
          </span>
        ) : (
          <>
            <span className="text-amber-700 dark:text-amber-400">
              {status.notReadyReason ?? "Not ready"}. Sign-in subscriptions
              can't route instantly.
            </span>
            <button
              className="inline-flex items-center gap-0.5 font-medium text-primary hover:underline focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring"
              data-testid={`settings-models-task-${task.id}-open-providers`}
              onClick={onOpenProviders}
              type="button"
            >
              Add one in Providers
              <ChevronRight aria-hidden="true" className="h-3.5 w-3.5" />
            </button>
          </>
        )}
      </div>
      {saveMutation.error ? (
        <p className="text-xs text-destructive" role="alert">
          {saveMutation.error instanceof Error
            ? saveMutation.error.message
            : String(saveMutation.error)}
        </p>
      ) : null}
    </div>
  );
}
