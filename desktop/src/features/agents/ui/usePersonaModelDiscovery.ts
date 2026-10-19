import * as React from "react";

import { useChatGptPlanStatus } from "@/features/agents/useChatGptPlan";
import { discoverAgentModels } from "@/shared/api/agentModels";
import type {
  AcpRuntimeCatalogEntry,
  AgentModelsResponse,
  HarnessProviderInfo,
} from "@/shared/api/types";
import type { EnvVarsValue } from "./EnvVarsEditor";
import {
  formatModelDiscoveryErrorStatus,
  type PersonaModelDiscoveryStatus,
} from "./personaModelDiscoveryStatus";
import type { PersonaModelOption } from "./agentConfigOptions";
import { providerRequiresExplicitModel } from "./agentConfigOptions";
import {
  HARNESS_NOT_SET_UP_MODEL_DESCRIPTION,
  HARNESS_UNAVAILABLE_MODEL_DESCRIPTION,
  harnessProviderOfModel,
  modelForHarnessProvider,
  modelOptionsForHarnessProvider,
} from "./harnessProviderOptions";
import {
  disambiguateModelLabels,
  resolveModelLabel,
} from "@/features/agents/lib/formatAgentModelLabel";

export const MODEL_DISCOVERY_LOADING_VALUE = "__model_discovery_loading__";

const MODEL_DISCOVERY_CREDENTIAL_DEBOUNCE_MS = 250;

function stableModelDiscoveryEnvKey(envVars: EnvVarsValue): string {
  return JSON.stringify(
    Object.entries(envVars).sort(([left], [right]) =>
      left.localeCompare(right),
    ),
  );
}

/**
 * True when a harness catalog entry is the harness's own "use my default"
 * row (e.g. Claude Code ships a literal `default` model id). Such entries
 * mean the same thing as leaving the model unset, so the UI merges them
 * into the single canonical default row instead of showing two rows for
 * one idea.
 */
function isHarnessDefaultModelEntry(model: { id: string }) {
  return model.id.trim().toLowerCase() === "default";
}

export function getDiscoveredPersonaModelOptions(
  response: AgentModelsResponse | null,
  provider: string,
): readonly PersonaModelOption[] | null {
  if (!response?.supportsSwitching || response.models.length === 0) {
    return null;
  }

  // One row per idea: the harness's own default catalog entry (if any) is
  // absorbed into the canonical default row. Selecting it keeps the stored
  // model unset — behaviorally identical, and it avoids two saved states
  // ("default" vs unset) that mean the same thing.
  const explicitModels = response.models.filter(
    (model) => !isHarnessDefaultModelEntry(model),
  );
  const harnessDefaultEntry = response.models.find(isHarnessDefaultModelEntry);
  const agentDefaultModel = response.agentDefaultModel?.trim();

  const defaultModelOption =
    providerRequiresExplicitModel(provider) && harnessDefaultEntry === undefined
      ? []
      : [
          {
            id: "",
            label:
              provider === "relay-mesh"
                ? "Default (auto)"
                : agentDefaultModel
                  ? `Default model (${resolveModelLabel(agentDefaultModel, null, provider)})`
                  : "Default model",
          },
        ];

  if (explicitModels.length === 0 && defaultModelOption.length === 0) {
    return null;
  }

  return [
    ...defaultModelOption,
    ...disambiguateModelLabels(
      explicitModels.map((model) => ({
        id: model.id,
        label:
          model.description === HARNESS_UNAVAILABLE_MODEL_DESCRIPTION
            ? `${resolveModelLabel(model.id, model.name, provider)} (not available in Hermes)`
            : model.description === HARNESS_NOT_SET_UP_MODEL_DESCRIPTION
              ? `${resolveModelLabel(model.id, model.name, provider)} (not set up in Hermes)`
              : resolveModelLabel(model.id, model.name, provider),
        ...(model.supportedEfforts?.length
          ? { supportedEfforts: model.supportedEfforts }
          : {}),
        ...(model.defaultEffort ? { defaultEffort: model.defaultEffort } : {}),
      })),
      provider,
    ),
  ];
}

/**
 * Returns a warning status when a discovery response resolves but contains no
 * usable model options (either because the harness does not support model
 * switching, or because it returned an empty model list). When options ARE
 * available, returns null so callers can clear any prior status.
 */
export function synthesizeEmptyDiscoveryStatus(
  response: AgentModelsResponse,
  provider: string,
): PersonaModelDiscoveryStatus | null {
  if (getDiscoveredPersonaModelOptions(response, provider) !== null) {
    return null;
  }
  const agentLabel = response.agentName.trim() || "This agent";
  return {
    message: `${agentLabel} reported no models. Check that the CLI is installed and signed in, then reopen this screen.`,
    tone: "warning",
  };
}

/**
 * True when a discovery response is worth caching.  Responses that yielded no
 * usable model options are intentionally excluded so that close → reopen
 * re-runs discovery, letting the user's CLI install or sign-in be reflected
 * without a hard refresh.
 */
export function isCacheableDiscoveryResponse(
  response: AgentModelsResponse,
  provider: string,
): boolean {
  return getDiscoveredPersonaModelOptions(response, provider) !== null;
}

/**
 * Pure derivation of the "discovery is still pending" flag exposed by the
 * hook.  Extracted so tests can verify resolved-but-empty responses do not
 * count as pending.
 */
export function deriveModelDiscoveryPending({
  modelDiscoveryLoading,
  modelDiscoveryKey,
  activeModelDiscoveryData,
  activeModelDiscoveryStatus,
}: {
  modelDiscoveryLoading: boolean;
  modelDiscoveryKey: string | null;
  activeModelDiscoveryData: AgentModelsResponse | null;
  activeModelDiscoveryStatus: PersonaModelDiscoveryStatus | null;
}): boolean {
  return (
    modelDiscoveryLoading ||
    (modelDiscoveryKey !== null &&
      activeModelDiscoveryData === null &&
      activeModelDiscoveryStatus === null)
  );
}

/**
 * True when discovery IPC resolved with a response that yielded no usable
 * model options. Distinct from a thrown/unavailable failure (data stays null).
 * Callers that omit the Model control or heal persisted values must gate on
 * this — not on `discoveredModelOptions === null` alone.
 */
export function isSuccessfulEmptyDiscovery({
  activeModelDiscoveryData,
  discoveredModelOptions,
  modelDiscoveryPending,
}: {
  activeModelDiscoveryData: AgentModelsResponse | null;
  discoveredModelOptions: readonly PersonaModelOption[] | null;
  modelDiscoveryPending: boolean;
}): boolean {
  return (
    !modelDiscoveryPending &&
    activeModelDiscoveryData !== null &&
    discoveredModelOptions === null
  );
}

/** The provider row for a harness that owns its providers (Hermes). */
export type HarnessProviderChoice = {
  providers: readonly HarnessProviderInfo[];
  /** The provider of the current model (the default one when unset). */
  selectedId: string | undefined;
  /** The model to store when the user picks `providerId`. */
  modelFor: (providerId: string) => string;
};

export function usePersonaModelDiscovery({
  agentPubkey,
  envVars,
  isCustomProviderEditing,
  model,
  modelFieldVisible,
  open,
  persistedModel,
  provider,
  selectedRuntime,
}: {
  /** The saved agent being edited, so a Hermes agent lists its own profile. */
  agentPubkey?: string;
  envVars: EnvVarsValue;
  isCustomProviderEditing: boolean;
  /** The current model choice; picks which harness provider's models show. */
  model?: string;
  modelFieldVisible: boolean;
  open: boolean;
  /** The model saved on the agent, kept listed even if no longer offered. */
  persistedModel?: string;
  provider: string;
  selectedRuntime: AcpRuntimeCatalogEntry | undefined;
}) {
  const [modelDiscoveryData, setModelDiscoveryData] =
    React.useState<AgentModelsResponse | null>(null);
  const [modelDiscoveryDataKey, setModelDiscoveryDataKey] = React.useState<
    string | null
  >(null);
  const [modelDiscoveryStatus, setModelDiscoveryStatus] =
    React.useState<PersonaModelDiscoveryStatus | null>(null);
  const [modelDiscoveryStatusKey, setModelDiscoveryStatusKey] = React.useState<
    string | null
  >(null);
  const [modelDiscoveryLoading, setModelDiscoveryLoading] =
    React.useState(false);
  const modelDiscoveryCacheRef = React.useRef(
    new Map<string, AgentModelsResponse>(),
  );
  const modelDiscoveryRequestRef = React.useRef(0);

  const trimmedProvider = provider.trim();
  const shouldDebounceModelDiscovery =
    providerRequiresExplicitModel(trimmedProvider);
  const discoveryAgentCommand = selectedRuntime?.command?.trim()
    ? selectedRuntime.command
    : null;
  // Narrow to the individual fields the effect consumes so a new object
  // reference from a React Query refetch (same data, unstable ref) does not
  // abandon and re-issue an in-flight discovery IPC call.
  const selectedRuntimeAvailability = selectedRuntime?.availability;
  const selectedRuntimeLabel = selectedRuntime?.label;
  const selectedRuntimeDefaultArgs = selectedRuntime?.defaultArgs;
  const selectedRuntimeDefinitionEnv = selectedRuntime?.definitionEnv;
  const canDiscoverModelOptions =
    open &&
    modelFieldVisible &&
    selectedRuntime?.availability === "available" &&
    discoveryAgentCommand !== null &&
    (!isCustomProviderEditing || trimmedProvider.length > 0);
  const modelDiscoveryEnvKey = React.useMemo(
    () => stableModelDiscoveryEnvKey(envVars),
    [envVars],
  );
  const chatGptPlan = useChatGptPlanStatus();
  const chatGptSignedIn =
    trimmedProvider === "openai-subscription"
      ? Boolean(chatGptPlan.data?.signedIn && chatGptPlan.data.planEnabled)
      : null;
  const modelDiscoveryArgsKey = JSON.stringify(
    selectedRuntime?.defaultArgs ?? [],
  );
  const modelDiscoveryKey = React.useMemo(() => {
    if (!canDiscoverModelOptions || discoveryAgentCommand === null) {
      return null;
    }

    return JSON.stringify({
      agentCommand: discoveryAgentCommand,
      agentArgs: modelDiscoveryArgsKey,
      provider: trimmedProvider,
      envVars: modelDiscoveryEnvKey,
      agentPubkey: agentPubkey ?? null,
      persistedModel: persistedModel?.trim() || null,
      // Reload the ChatGPT plan's model list right after sign-in or sign-out.
      chatGptSignedIn: chatGptSignedIn,
    });
  }, [
    agentPubkey,
    canDiscoverModelOptions,
    chatGptSignedIn,
    discoveryAgentCommand,
    modelDiscoveryArgsKey,
    modelDiscoveryEnvKey,
    persistedModel,
    trimmedProvider,
  ]);

  React.useEffect(() => {
    if (modelDiscoveryKey === null || discoveryAgentCommand === null) {
      modelDiscoveryRequestRef.current += 1;
      setModelDiscoveryData(null);
      setModelDiscoveryDataKey(null);
      // When the runtime exists but is not available, surface a status message
      // so the model dropdown explains why no live models can be loaded.
      if (
        selectedRuntimeAvailability != null &&
        selectedRuntimeAvailability !== "available"
      ) {
        setModelDiscoveryStatus(
          formatModelDiscoveryErrorStatus(
            new Error(`Runtime not available: ${selectedRuntimeAvailability}`),
            trimmedProvider,
            selectedRuntimeLabel,
          ),
        );
        setModelDiscoveryStatusKey(null);
      } else {
        setModelDiscoveryStatus(null);
        setModelDiscoveryStatusKey(null);
      }
      setModelDiscoveryLoading(false);
      return;
    }

    const requestId = modelDiscoveryRequestRef.current + 1;
    modelDiscoveryRequestRef.current = requestId;
    const activeAgentCommand = discoveryAgentCommand;
    const activeModelDiscoveryKey = modelDiscoveryKey;
    const cached = modelDiscoveryCacheRef.current.get(activeModelDiscoveryKey);
    if (cached) {
      setModelDiscoveryData(cached);
      setModelDiscoveryDataKey(activeModelDiscoveryKey);
      setModelDiscoveryStatus(
        synthesizeEmptyDiscoveryStatus(cached, trimmedProvider),
      );
      setModelDiscoveryStatusKey(activeModelDiscoveryKey);
      setModelDiscoveryLoading(false);
      return;
    }

    setModelDiscoveryData(null);
    setModelDiscoveryDataKey(null);
    setModelDiscoveryStatus(null);
    setModelDiscoveryStatusKey(activeModelDiscoveryKey);
    setModelDiscoveryLoading(true);
    function runModelDiscovery() {
      void discoverAgentModels({
        agentCommand: activeAgentCommand,
        agentArgs: selectedRuntimeDefaultArgs ?? [],
        provider: trimmedProvider || undefined,
        envVars,
        definitionEnv: selectedRuntimeDefinitionEnv ?? {},
        ...(agentPubkey ? { agentPubkey } : {}),
        ...(persistedModel?.trim()
          ? { persistedModel: persistedModel.trim() }
          : {}),
      })
        .then((response) => {
          if (modelDiscoveryRequestRef.current !== requestId) {
            return;
          }
          // Only cache responses that yielded usable model options.  An
          // empty/no-switching result gets the "reopen this screen" warning,
          // and closing → reopening the dialog must re-run discovery so the
          // user's CLI-install/sign-in is actually reflected.
          if (isCacheableDiscoveryResponse(response, trimmedProvider)) {
            modelDiscoveryCacheRef.current.set(
              activeModelDiscoveryKey,
              response,
            );
          }
          setModelDiscoveryData(response);
          setModelDiscoveryDataKey(activeModelDiscoveryKey);
          setModelDiscoveryStatus(
            synthesizeEmptyDiscoveryStatus(response, trimmedProvider),
          );
          setModelDiscoveryStatusKey(activeModelDiscoveryKey);
        })
        .catch((error) => {
          if (modelDiscoveryRequestRef.current !== requestId) {
            return;
          }
          setModelDiscoveryData(null);
          setModelDiscoveryDataKey(null);
          setModelDiscoveryStatus(
            formatModelDiscoveryErrorStatus(
              error,
              trimmedProvider,
              selectedRuntimeLabel,
            ),
          );
          setModelDiscoveryStatusKey(activeModelDiscoveryKey);
        })
        .finally(() => {
          if (modelDiscoveryRequestRef.current === requestId) {
            setModelDiscoveryLoading(false);
          }
        });
    }

    if (!shouldDebounceModelDiscovery) {
      runModelDiscovery();
      return;
    }

    const timeout = window.setTimeout(
      runModelDiscovery,
      MODEL_DISCOVERY_CREDENTIAL_DEBOUNCE_MS,
    );

    return () => {
      window.clearTimeout(timeout);
      if (modelDiscoveryRequestRef.current === requestId) {
        modelDiscoveryRequestRef.current += 1;
        setModelDiscoveryLoading(false);
      }
    };
  }, [
    agentPubkey,
    discoveryAgentCommand,
    envVars,
    modelDiscoveryKey,
    selectedRuntimeAvailability,
    selectedRuntimeDefaultArgs,
    selectedRuntimeDefinitionEnv,
    selectedRuntimeLabel,
    persistedModel,
    shouldDebounceModelDiscovery,
    trimmedProvider,
  ]);

  const activeModelDiscoveryData =
    modelDiscoveryKey !== null && modelDiscoveryDataKey === modelDiscoveryKey
      ? modelDiscoveryData
      : null;
  const activeModelDiscoveryStatus =
    modelDiscoveryKey === null
      ? modelDiscoveryStatus
      : modelDiscoveryStatusKey === modelDiscoveryKey
        ? modelDiscoveryStatus
        : null;
  const allDiscoveredModelOptions = React.useMemo(
    () =>
      getDiscoveredPersonaModelOptions(
        activeModelDiscoveryData,
        trimmedProvider,
      ),
    [activeModelDiscoveryData, trimmedProvider],
  );
  // A harness that owns its providers lists models of every provider the user
  // set up in it; show only the current model's provider's models.
  const harnessProviders = activeModelDiscoveryData?.harnessProviders;
  const harnessProviderChoice = React.useMemo<HarnessProviderChoice | null>(
    () =>
      harnessProviders && harnessProviders.length > 0
        ? {
            providers: harnessProviders,
            selectedId: harnessProviderOfModel(model ?? "", harnessProviders),
            modelFor: (providerId) =>
              modelForHarnessProvider(
                providerId,
                allDiscoveredModelOptions ?? [],
                harnessProviders,
              ),
          }
        : null,
    [allDiscoveredModelOptions, harnessProviders, model],
  );
  const discoveredModelOptions = React.useMemo(() => {
    if (allDiscoveredModelOptions === null || harnessProviderChoice === null) {
      return allDiscoveredModelOptions;
    }
    const scoped = modelOptionsForHarnessProvider(
      allDiscoveredModelOptions,
      harnessProviderChoice.providers,
      harnessProviderChoice.selectedId,
    );
    return scoped.length > 0 ? scoped : allDiscoveredModelOptions;
  }, [allDiscoveredModelOptions, harnessProviderChoice]);
  const modelDiscoveryPending = deriveModelDiscoveryPending({
    modelDiscoveryLoading,
    modelDiscoveryKey,
    activeModelDiscoveryData,
    activeModelDiscoveryStatus,
  });
  const modelDiscoverySuccessfulEmpty = isSuccessfulEmptyDiscovery({
    activeModelDiscoveryData,
    discoveredModelOptions,
    modelDiscoveryPending,
  });

  return {
    discoveredModelOptions,
    harnessProviderChoice,
    modelDiscoveryLoading: modelDiscoveryPending,
    modelDiscoveryStatus:
      modelDiscoveryPending || discoveredModelOptions !== null
        ? null
        : activeModelDiscoveryStatus,
    modelDiscoverySuccessfulEmpty,
  };
}
