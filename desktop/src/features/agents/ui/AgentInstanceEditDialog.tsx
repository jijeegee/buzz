import { isRelayRemovedError } from "@/features/agents/managedAgentRelayCleanup";
import * as React from "react";
import { useQueryClient } from "@tanstack/react-query";
import { ChevronDown } from "lucide-react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { toast } from "sonner";

import {
  agentConfigSurfaceQueryKey,
  useAcpCommandsQuery,
  useAcpRuntimesQuery,
  useAgentConfigSurface,
  useBakedBuildEnvKeysQuery,
  useStartManagedAgentMutation,
  useUpdateManagedAgentMutation,
} from "@/features/agents/hooks";
import { useAgentAccessOwnerOnlyQuery } from "@/features/agents/useAgentAccessOwnerOnly";
import { isManagedAgentActive } from "@/features/agents/lib/managedAgentControlActions";
import type {
  ManagedAgent,
  RespondToMode,
  UpdateManagedAgentInput,
} from "@/shared/api/types";
import type { EditAgentFocusTarget } from "@/features/agents/openEditAgentEvent";
import { cn } from "@/shared/lib/cn";
import { Button } from "@/shared/ui/button";
import { ChooserDialogContent } from "@/shared/ui/chooser-dialog-content";
import { Dialog } from "@/shared/ui/dialog";
import { Input } from "@/shared/ui/input";
import { setManagedAgentAutoRestart } from "@/shared/api/tauriManagedAgents";
import { EditAgentAdvancedFields } from "./EditAgentAdvancedFields";
import { KuzzAgentModelTuningFields } from "./kuzzAgentModelTuningFields";
import { isKuzzAgentRuntime } from "./kuzzAgentConfig";
import {
  ADVANCED_FIELDS_MOTION_TRANSITION,
  AUTO_PROVIDER_DROPDOWN_VALUE,
  buildHiddenProviderIds,
  CUSTOM_PROVIDER_DROPDOWN_VALUE,
  formatRuntimeOptionLabel,
  getDefaultLlmModelLabel,
  getPersonaProviderOptions,
  isMissingRequiredDropdownField,
  NO_RUNTIME_DROPDOWN_VALUE,
  PERSONA_FIELD_CONTROL_CLASS,
  PERSONA_FIELD_SHELL_CLASS,
  runtimeSupportsLlmProviderSelection,
  shouldClearKnownModelForSelectionScope,
  sortPersonaRuntimes,
  type PersonaDropdownOption,
} from "./agentConfigOptions";
import {
  modelDropdownOptions as buildModelDropdownOptions,
  relayMeshModelPickerState,
} from "./relayMeshModelPicker";
import {
  computeEditAgentFormValidity,
  envVarsEqual,
  isEditAgentProviderSaveValid,
  resolveAgentCommandUpdate,
  resolveEffortSubmission,
  resolveRuntimeProviderCapability,
} from "./personaRuntimeModel";
import {
  selectionOnModelDropdownChange,
  selectionOnProviderDropdownChange,
  selectionOnRuntimeChange,
  type RuntimeModelProviderSelection,
} from "./runtimeModelProviderSelection";
import { AgentCreationPreview } from "./AgentCreationPreview";
import { OwnerOnlyAccessField } from "./OwnerOnlyAccessField";
import type { EnvVarsValue } from "./EnvVarsEditor";
import { useRequiredCredentialState } from "./useRequiredCredentialState";
import { RunOnSummarySection } from "./RunOnSummarySection";
import { PersonaDropdownField } from "./PersonaDropdownField";
import {
  MODEL_DISCOVERY_LOADING_VALUE,
  usePersonaModelDiscovery,
} from "./usePersonaModelDiscovery";
import { EditAgentProviderModelFields } from "./EditAgentProviderModelFields";
import {
  getBakedModelInheritLabel,
  getBakedProviderInheritLabel,
} from "./bakedEnvHelpers";
import { useAgentDialogDefaults } from "./useAgentDialogDefaults";
import { AgentAiDefaultsNotice } from "./AgentAiDefaults";
import { AgentDefaultsDialog } from "./AgentDefaultsDialog";
import { useProviderApiKeyFieldState } from "./providerApiKeyFieldState";
import { resolveModelFieldStatusMessage } from "./agentConfigControls";
import { AdvancedRequiredBadge } from "./AdvancedRequiredBadge";
import { showAgentProfileSyncWarning } from "./agentProfileSyncWarning";
import { AddCustomHarnessDialog } from "./AddCustomHarnessDialog";
import {
  ADD_CUSTOM_HARNESS_OPTION,
  runtimeDropdownAction,
  usePendingHarnessSelection,
} from "./addCustomHarness";

export function AgentInstanceEditDialog({
  agent,
  initialFocus,
  open,
  onOpenChange,
  onUpdated,
}: {
  agent: ManagedAgent;
  /** Optional field to scroll/focus when the dialog opens from a card deep-link. */
  initialFocus?: EditAgentFocusTarget;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onUpdated?: (agent: ManagedAgent) => void;
}) {
  const updateMutation = useUpdateManagedAgentMutation();
  const startMutation = useStartManagedAgentMutation();
  const queryClient = useQueryClient();
  // Gate the full Save sequence, including standalone setters, with isSaving.
  const [isSaving, setIsSaving] = React.useState(false);
  // Keep standalone-setter failures visible so the user can retry Save.
  const [setterError, setSetterError] = React.useState<Error | null>(null);
  const runtimesQuery = useAcpRuntimesQuery({ enabled: open });
  const acpCommandsQuery = useAcpCommandsQuery({ enabled: open });
  const configSurfaceQuery = useAgentConfigSurface(open ? agent.pubkey : null);
  const runtimes = runtimesQuery.data ?? [];

  const [name, setName] = React.useState(agent.name);
  const [description, setDescription] = React.useState(agent.description ?? "");
  const [aiDefaultsOpen, setAiDefaultsOpen] = React.useState(false);
  const aiDefaultsTriggerRef = React.useRef<HTMLButtonElement>(null);
  const [acpCommand, setAcpCommand] = React.useState(agent.acpCommand);
  const [agentCommand, setAgentCommand] = React.useState(agent.agentCommand);
  const [originalAgentCommand, setOriginalAgentCommand] = React.useState(
    agent.agentCommand,
  );
  const [agentArgs, setAgentArgs] = React.useState(agent.agentArgs.join(","));
  const [parallelism, setParallelism] = React.useState(
    String(agent.parallelism),
  );
  const [systemPrompt, setSystemPrompt] = React.useState(
    agent.systemPrompt ?? "",
  );
  const [model, setModel] = React.useState(agent.model ?? "");
  const [isCustomModelEditing, setIsCustomModelEditing] = React.useState(false);
  const [provider, setProvider] = React.useState(agent.provider ?? "");
  const [isCustomProviderEditing, setIsCustomProviderEditing] =
    React.useState(false);
  const [envVars, setEnvVars] = React.useState<EnvVarsValue>(agent.envVars);
  const [autoRestartOnConfigChange, setAutoRestartOnConfigChange] =
    React.useState(agent.autoRestartOnConfigChange);
  // Effort picker is Save-gated (resolveEffortSubmission / handleSubmit, PR
  // #4625); `effortTouched` = user picked, so an untouched Save writes nothing.
  const [effortLevel, setEffortLevel] = React.useState<string | null>(null);
  const effortTouched = React.useRef(false);
  const [respondTo, setRespondTo] = React.useState<RespondToMode>(
    agent.respondTo,
  );
  const [respondToAllowlist, setRespondToAllowlist] = React.useState<string[]>(
    agent.respondToAllowlist,
  );
  const [showAdvancedFields, setShowAdvancedFields] = React.useState(false);
  const [avatarUrl, setAvatarUrl] = React.useState(agent.avatarUrl ?? "");
  const [isAvatarUploadPending, setIsAvatarUploadPending] =
    React.useState(false);
  const [isAddHarnessOpen, setIsAddHarnessOpen] = React.useState(false);
  const shouldReduceMotion = useReducedMotion();

  // Runtime selector: defaults to "custom" until the dialog opens and the
  // catalog loads. The open-effect re-derives the correct id from the catalog.
  const [selectedRuntimeId, setSelectedRuntimeId] = React.useState("custom");

  // Tracks whether the user has made an in-dialog runtime selection.
  const runtimeTouched = React.useRef(false);

  // Reset form state only when the dialog opens or when switching to a different agent.
  // biome-ignore lint/correctness/useExhaustiveDependencies: intentional — including agent fields would re-fire on every 5s poll and wipe edits
  React.useEffect(() => {
    if (open) {
      setName(agent.name);
      setDescription(agent.description ?? "");
      setAcpCommand(agent.acpCommand);
      setAgentCommand(agent.agentCommand);
      setOriginalAgentCommand(agent.agentCommand);
      setAgentArgs(agent.agentArgs.join(","));
      setParallelism(String(agent.parallelism));
      setSystemPrompt(agent.systemPrompt ?? "");
      setModel(agent.model ?? "");
      setIsCustomModelEditing(false);
      setProvider(agent.provider ?? "");
      setIsCustomProviderEditing(false);
      setEnvVars(agent.envVars);
      setAutoRestartOnConfigChange(agent.autoRestartOnConfigChange);
      setEffortLevel(null);
      effortTouched.current = false;
      setSetterError(null);
      setRespondTo(agent.respondTo);
      setRespondToAllowlist(agent.respondToAllowlist);
      setAvatarUrl(agent.avatarUrl ?? "");
      setShowAdvancedFields(false);
      setIsAvatarUploadPending(false);
      setIsAddHarnessOpen(false);
      runtimeTouched.current = false;
      const matched =
        runtimes.find((r) => r.command?.trim() === agent.agentCommand.trim()) ??
        runtimes.find((r) => r.id === agent.agentCommand.trim());
      setSelectedRuntimeId(matched ? matched.id : "custom");
      updateMutation.reset();
    }
  }, [open, agent.pubkey]);

  // Re-derive the runtime id when the catalog loads.
  React.useEffect(() => {
    if (!open || runtimeTouched.current || runtimes.length === 0) {
      return;
    }
    const matched =
      runtimes.find((r) => r.command?.trim() === agent.agentCommand.trim()) ??
      runtimes.find((r) => r.id === agent.agentCommand.trim());
    if (matched) {
      setSelectedRuntimeId(matched.id);
    }
  }, [open, runtimes, agent.agentCommand]);

  // Build the sorted runtime catalog for the dropdown.
  const sortedRuntimes = React.useMemo(
    () => sortPersonaRuntimes(runtimes),
    [runtimes],
  );

  const selectedRuntime = React.useMemo(
    () => runtimes.find((r) => r.id === selectedRuntimeId),
    [runtimes, selectedRuntimeId],
  );

  const runtimeDropdownValue = selectedRuntimeId || NO_RUNTIME_DROPDOWN_VALUE;

  const runtimeDropdownOptions: PersonaDropdownOption[] = React.useMemo(() => {
    const options: PersonaDropdownOption[] = [
      ...sortedRuntimes.map((candidate) => ({
        label: formatRuntimeOptionLabel(candidate),
        value: candidate.id,
      })),
      { label: "Custom command", value: "custom" },
    ];
    if (
      selectedRuntimeId &&
      selectedRuntimeId !== "custom" &&
      !options.some((o) => o.value === selectedRuntimeId)
    ) {
      options.push({
        label: `${selectedRuntimeId} (current)`,
        value: selectedRuntimeId,
      });
    }
    options.push(ADD_CUSTOM_HARNESS_OPTION);
    return options;
  }, [sortedRuntimes, selectedRuntimeId]);

  // Resolve the dialog-opening command as the catalog loads. Edit-state runtime
  // ids mutate during selection changes and cannot identify the original state.
  const originalRuntimeSupportsProvider = React.useMemo(() => {
    const originalCommand = originalAgentCommand.trim();
    const matched =
      runtimes.find((r) => r.command?.trim() === originalCommand) ??
      runtimes.find((r) => r.id === originalCommand);
    return runtimeSupportsLlmProviderSelection(matched?.id ?? "");
  }, [runtimes, originalAgentCommand]);

  // The runtime id active after submit — the single prospective id feeding BOTH
  // the block-save gate and submit so they always agree.
  const prospectiveRuntimeId = selectedRuntime?.id ?? selectedRuntimeId;

  const llmProviderFieldVisible =
    runtimeSupportsLlmProviderSelection(prospectiveRuntimeId);

  const prospectiveRuntime = runtimes.find(
    (r) => r.id === prospectiveRuntimeId,
  );
  // The effort the NEXT spawn launches, which is what the picker shows and
  // what an explicit pick is diffed against (`originalEffortLevel`). Before a
  // runtime switch that is the config surface's effective `thinkingEffort`
  // (resolved across env tiers under the running runtime's contract). After a
  // switch the surface is the OLD runtime's resolution — a Goose
  // `GOOSE_THINKING_EFFORT` tier would be shown for a runtime that never reads
  // it — so it is the stored column (`agent.effortLevel`), the only tier an
  // ACP thought-level runtime launches (`KUZZ_ACP_EFFORT_LEVEL`).
  const launchEffortLevel = runtimeTouched.current
    ? agent.effortLevel
    : (configSurfaceQuery.data?.normalized.thinkingEffort?.value ?? null);
  const runtimeCatalogStatus = runtimesQuery.isLoading
    ? ("loading" as const)
    : runtimesQuery.isError
      ? ("error" as const)
      : ("ready" as const);

  // One-shot focus: when the dialog opens from a card deep-link, scroll and
  // focus the relevant field. The effect re-runs when `llmProviderFieldVisible`
  // changes so a provider-field focus request fires once the field materializes.
  const normalizedFieldFocusFiredRef = React.useRef(false);
  // biome-ignore lint/correctness/useExhaustiveDependencies: intentional — reset guard on these three; llmProviderFieldVisible drives the focus attempt below
  React.useEffect(() => {
    normalizedFieldFocusFiredRef.current = false;
  }, [open, initialFocus, agent.pubkey]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: intentional — llmProviderFieldVisible is the availability signal that re-triggers the focus attempt; agent.pubkey handles agent-switch
  React.useEffect(() => {
    if (!open || !initialFocus) return;
    if (initialFocus.type !== "normalized_field") return;
    if (normalizedFieldFocusFiredRef.current) return;

    const targetId =
      initialFocus.field === "provider"
        ? "edit-agent-llm-provider"
        : "edit-agent-model";
    const el = document.getElementById(targetId);
    if (!(el instanceof HTMLElement)) return;

    normalizedFieldFocusFiredRef.current = true;

    const id = requestAnimationFrame(() => {
      el.scrollIntoView({ block: "nearest" });
      el.focus();
    });

    return () => cancelAnimationFrame(id);
  }, [open, initialFocus, agent.pubkey, llmProviderFieldVisible]);

  // Provider + model + env to PERSIST on submit — also fed to the credential
  // gate so gate, saved record, and spawn all agree on one value. The agent
  // owns every field: nothing is inherited from the preset it was made from.
  const submission = React.useMemo(
    () => ({
      provider: provider.trim() || null,
      model: model.trim() || null,
      envVars,
    }),
    [provider, model, envVars],
  );

  const {
    globalConfig,
    inheritedDefaults: {
      provider: inheritedProviderDefault,
      model: inheritedModelDefault,
    },
    inheritedEnvVars: inheritedEnvVarsForAdvanced,
  } = useAgentDialogDefaults({ open });

  // Runtime/provider-required credential state for the PROSPECTIVE post-submit runtime.
  // globalProvider/globalEnvVars: fallback for empty per-agent provider; keys satisfied globally don't block Save.
  const { requiredEnvKeys, fileSatisfiedEnvKeys, requiredEnvKeyMissing } =
    useRequiredCredentialState({
      open,
      prospectiveRuntimeId,
      provider: submission.provider ?? "",
      globalProvider: inheritedProviderDefault.value,
      envVars: submission.envVars,
      globalEnvVars: globalConfig.env_vars,
    });

  const { data: bakedEnvKeys } = useBakedBuildEnvKeysQuery({ enabled: open });
  const { data: agentAccessOwnerOnly } = useAgentAccessOwnerOnlyQuery({
    enabled: open,
  });

  // Merge global env as the base layer so credential keys satisfied via global
  // config (e.g. ANTHROPIC_API_KEY) are available to model discovery.
  // Agent-local env takes precedence, matching the agent → global → file
  // spawn-path precedence.
  const envVarsForDiscovery = React.useMemo(
    () => ({ ...globalConfig.env_vars, ...submission.envVars }),
    [globalConfig.env_vars, submission.envVars],
  );
  const effectiveProvider =
    (submission.provider ?? "").trim() || inheritedProviderDefault.value;
  const providerForDiscovery = llmProviderFieldVisible ? effectiveProvider : "";

  const {
    discoveredModelOptions,
    harnessProviderChoice,
    modelDiscoveryLoading,
    modelDiscoveryStatus,
  } = usePersonaModelDiscovery({
    envVars: envVarsForDiscovery,
    isCustomProviderEditing,
    model,
    agentPubkey: agent.pubkey,
    persistedModel: agent.model ?? undefined,
    modelFieldVisible: true,
    open,
    provider: providerForDiscovery,
    selectedRuntime,
  });

  // D2/D3: the top-level API key owns display while the readiness gate keeps the
  // complete required-key list; advancedRequiredEnvKeys drives EnvVarsEditor
  // display only.
  const apiKeyFieldState = useProviderApiKeyFieldState({
    bakedEnvKeys,
    effectiveEnvVars: submission.envVars,
    envVars,
    fileSatisfiedEnvKeys,
    globalEnvVars: globalConfig.env_vars,
    provider: effectiveProvider,
    requiredEnvKeys,
  });
  const {
    advancedRequiredEnvKeys,
    inheritedLabel: apiKeyInheritedLabel,
    isInherited: apiKeyIsInherited,
    isRequired: apiKeyIsRequired,
    secretEnvVar: topLevelSecretEnvVar,
    value: apiKeyValue,
  } = apiKeyFieldState;
  // Clear model when provider scope changes and current model is no longer valid.
  React.useEffect(() => {
    if (
      !open ||
      isCustomModelEditing ||
      !shouldClearKnownModelForSelectionScope({
        model,
        provider: providerForDiscovery,
        runtime: selectedRuntime?.id ?? selectedRuntimeId,
      })
    ) {
      return;
    }

    setModel("");
    setIsCustomModelEditing(false);
  }, [
    isCustomModelEditing,
    model,
    open,
    providerForDiscovery,
    selectedRuntime,
    selectedRuntimeId,
  ]);

  const selection: RuntimeModelProviderSelection = {
    provider,
    model,
    isCustomProviderEditing,
    isCustomModelEditing,
    envVars,
  };

  function applySelection(next: RuntimeModelProviderSelection) {
    setProvider(next.provider);
    setModel(next.model);
    setIsCustomProviderEditing(next.isCustomProviderEditing);
    setIsCustomModelEditing(next.isCustomModelEditing);
    setEnvVars(next.envVars);
  }

  function handleRuntimeDropdownChange(nextValue: string) {
    const action = runtimeDropdownAction(nextValue);
    if (action.kind === "add-custom-harness") {
      setIsAddHarnessOpen(true);
      return;
    }
    const nextRuntimeId = action.runtimeId;
    const previousRuntimeId = selectedRuntimeId;
    const nextRuntime = runtimes.find((r) => r.id === nextRuntimeId);

    // Mark that the user has made an explicit runtime choice. The catalog-arrival
    // effect will no longer overwrite selectedRuntimeId after this point.
    runtimeTouched.current = true;
    const resolvedRuntimeId = nextRuntimeId || "custom";
    setSelectedRuntimeId(resolvedRuntimeId);
    effortTouched.current = false;
    setEffortLevel(null);

    // When switching to a catalog-known runtime, update the agent command to
    // its resolved command so the command field stays consistent.
    if (nextRuntime?.command) {
      setAgentCommand(nextRuntime.command);
      const newArgs = nextRuntime.defaultArgs.join(",");
      setAgentArgs(newArgs);
    }

    applySelection(
      selectionOnRuntimeChange(selection, {
        previousRuntime: previousRuntimeId,
        nextRuntime: nextRuntime?.id ?? nextRuntimeId,
        nextRuntimeSupportsChatgptSubscription:
          nextRuntime?.supportsChatgptSubscription,
        nextRuntimeCanChooseProvider: runtimeSupportsLlmProviderSelection(
          nextRuntime?.id ?? nextRuntimeId,
        ),
        lockedRuntimeReset: "full",
      }),
    );
  }

  // Routed through the normal change handler so a harness registered inline
  // pins its command and resets model/provider like a hand-picked one. Scoped
  // to `open` so a pending id can't outlive the dialog that started the
  // registration.
  const selectSavedHarness = usePendingHarnessSelection(
    runtimes,
    handleRuntimeDropdownChange,
    open,
  );

  function handleProviderDropdownChange(nextValue: string) {
    const nextProvider =
      nextValue === AUTO_PROVIDER_DROPDOWN_VALUE ? "" : nextValue;
    if (nextProvider === "relay-mesh" && selectedRuntimeId !== "kuzz-agent") {
      handleRuntimeDropdownChange("kuzz-agent");
    }
    const nextSelection = selectionOnProviderDropdownChange(selection, {
      runtime:
        nextProvider === "relay-mesh"
          ? "kuzz-agent"
          : (selectedRuntime?.id ?? selectedRuntimeId),
      nextValue,
      clearModelWhenApiKeyMissing: false,
    });
    applySelection({
      ...nextSelection,
      model: nextProvider === "relay-mesh" ? "auto" : nextSelection.model,
    });
  }

  function handleModelDropdownChange(nextValue: string) {
    applySelection(
      selectionOnModelDropdownChange(selection, {
        nextValue,
        clearKnownModelOnCustomEntry: false,
        isModelCustom: false,
      }),
    );
  }

  function handleOpenChange(next: boolean) {
    // Reject user-originated dismissals (Escape, overlay, close-X, Cancel) while
    // a Save is in flight — the in-flight setters must not commit to a closed dialog.
    // The success path calls onOpenChange(false) directly, bypassing this guard.
    if (!next && isSaving) return;
    onOpenChange(next);
  }

  const providerValid = isEditAgentProviderSaveValid({
    llmProviderFieldVisible,
    currentProvider: provider,
    originalProvider: agent.provider,
    globalProvider: inheritedProviderDefault.value,
    originalRuntimeSupportsProvider,
  });

  const canSubmit =
    computeEditAgentFormValidity({
      name,
      parallelism,
      agentAcpCommand: agent.acpCommand,
      acpCommand,
      respondTo,
      respondToAllowlistLength: respondToAllowlist.length,
      selectedRuntimeId,
      agentCommand,
      requiredEnvKeyMissing,
    }) &&
    providerValid &&
    !isSaving &&
    !isAvatarUploadPending;

  async function handleSubmit() {
    setIsSaving(true);
    setSetterError(null);
    try {
      const parsedParallelism = Number.parseInt(parallelism, 10);
      const parsedArgs = agentArgs
        .split(",")
        .map((v) => v.trim())
        .filter((v) => v.length > 0);
      const normalizedModel = submission.model;

      // Send the command only when it changed (see resolveAgentCommandUpdate).
      const agentCommandUpdate = resolveAgentCommandUpdate({
        agentCommand,
        originalAgentCommand: agent.agentCommand,
      });

      // Classify the effective post-submit runtime's provider capability as a
      // tri-state: "capable" persists the provider, "locked" clears it (only
      // when we KNOW it's provider-locked, e.g. Claude), "unknown" OMITS it so a
      // transient/custom state never becomes a destructive write. Resolved
      // STATICALLY (by id) so a not-yet-loaded catalog can't misclassify a known
      // runtime as "unknown" — see resolveRuntimeProviderCapability. The runtime
      // id is the shared prospectiveRuntimeId, so submit and the block-save gate
      // always agree on which runtime is being saved.
      const providerRuntimeCapability = resolveRuntimeProviderCapability(
        prospectiveRuntimeId,
        runtimeSupportsLlmProviderSelection(prospectiveRuntimeId),
      );

      // Provider + env to persist — the same snapshot the credential gate
      // validates, so gate ↔ record ↔ spawn all agree.
      const normalizedSubmitProvider = submission.provider;
      const submitEnvVars = submission.envVars;
      const normalizedDescription = description.trim() || null;
      const input: UpdateManagedAgentInput = {
        pubkey: agent.pubkey,
        name: name.trim() !== agent.name ? name.trim() : undefined,
        description:
          normalizedDescription !== (agent.description ?? null)
            ? normalizedDescription
            : undefined,
        // relayUrl deliberately never submitted: the legacy per-record pin is
        // ignored (#2122) and the stored value is preserved as-is.
        acpCommand:
          acpCommand.trim() !== agent.acpCommand
            ? acpCommand.trim()
            : undefined,
        agentCommand: agentCommandUpdate,
        agentArgs:
          parsedArgs.join(",") !== agent.agentArgs.join(",")
            ? parsedArgs
            : undefined,
        parallelism:
          parsedParallelism > 0 && parsedParallelism !== agent.parallelism
            ? parsedParallelism
            : undefined,
        systemPrompt:
          (systemPrompt.trim() || null) !== agent.systemPrompt
            ? systemPrompt.trim() || null
            : undefined,
        model:
          normalizedModel !== (agent.model ?? null)
            ? normalizedModel
            : undefined,
        // Tri-state provider persistence keyed on providerRuntimeCapability
        // (see the classification comment above for the capable/locked/unknown
        // contract). llmProviderFieldVisible is UX visibility only; not used here.
        provider:
          providerRuntimeCapability === "capable"
            ? normalizedSubmitProvider !== (agent.provider ?? null)
              ? normalizedSubmitProvider
              : undefined
            : providerRuntimeCapability === "locked"
              ? (agent.provider ?? null) !== null
                ? null
                : undefined
              : undefined, // "unknown" → omit always
        envVars: envVarsEqual(submitEnvVars, agent.envVars)
          ? undefined
          : submitEnvVars,
        respondTo: respondTo !== agent.respondTo ? respondTo : undefined,
        // The allowlist is preserved across mode toggles in local UI state
        // (so a user can flip away from allowlist and back without losing
        // their entries), but we only send it on the wire when (a) it
        // actually changed, AND (b) the saved mode will need it. Sending
        // an allowlist while switching to a non-allowlist mode would be
        // harmless server-side, but it's noise in the persisted record.
        respondToAllowlist:
          respondTo === "allowlist" &&
          respondToAllowlist.join(",") !== agent.respondToAllowlist.join(",")
            ? respondToAllowlist
            : undefined,
      };

      // Resolve effort before the update so access-change restarts can
      // snapshot and launch the NEW effort value atomically.
      const effortSubmission = resolveEffortSubmission({
        effortLevel,
        originalEffortLevel: launchEffortLevel,
      });
      // Include effort in the locked update when touched (tri-state: absent =
      // don't touch; null = clear; string = set). Only when effortSubmission.persist.
      if (effortTouched.current && effortSubmission.persist) {
        input.effortLevel = effortSubmission.level;
      }

      const result = await updateMutation.mutateAsync(input);

      // Standalone setters — sequenced after the locked update resolves so the
      // dialog remains fully gated (isSaving) for the COMPLETE Save transaction.
      // A failure here surfaces as setterError (retryable) and aborts before
      // close, keeping the dialog open so the user can retry Save.
      try {
        if (autoRestartOnConfigChange !== agent.autoRestartOnConfigChange) {
          // Mirrors start-on-app-launch; not part of UpdateManagedAgentInput so
          // the frozen update shape stays frozen.
          await setManagedAgentAutoRestart(
            agent.pubkey,
            autoRestartOnConfigChange,
          );
        }
        // Effort disk write happened inside the locked update. Only need to
        // invalidate the cache here (when effortTouched && effortSubmission.persist).
        // If effort was not included (!effortSubmission.persist), nothing to do.
        if (effortTouched.current && effortSubmission.persist) {
          // Disk write already done; invalidate so the panel tier reflects it.
          await queryClient.invalidateQueries({
            queryKey: agentConfigSurfaceQueryKey(agent.pubkey),
          });
        }
      } catch (e) {
        setSetterError(e instanceof Error ? e : new Error("Failed to save"));
        return;
      }

      showAgentProfileSyncWarning(result.agent.name, result.profileSyncError);
      // Close via onOpenChange directly — handleOpenChange guards against
      // mid-save dismissal and must not block the intentional post-success close.
      onOpenChange(false);
      onUpdated?.(result.agent);
      // The auto-restart policy deliberately never fires for a stopped or
      // failing agent (a broken agent must not auto-loop), so an edit meant
      // to FIX one silently waits for a manual start. Offer that start
      // explicitly instead of relying on the user to know the policy.
      if (!isManagedAgentActive(result.agent)) {
        const startedName = result.agent.name;
        toast(`${startedName} saved while stopped.`, {
          action: {
            label: "Start now",
            onClick: () => {
              startMutation.mutate(result.agent.pubkey, {
                onSuccess: () => toast.success(`${startedName} started.`),
                onError: (error) =>
                  isRelayRemovedError(error) ||
                  toast.error(
                    `${startedName} failed to start${error instanceof Error ? `: ${error.message}` : "."}`,
                  ),
              });
            },
          },
        });
      }
    } catch {
      // React Query stores the update error; keep dialog open and render it inline.
    } finally {
      setIsSaving(false);
    }
  }

  // Model and provider field derived state
  const normalizedConfig = configSurfaceQuery.data?.normalized;
  const modelRequired = isMissingRequiredDropdownField(
    normalizedConfig?.model,
    model,
  );
  const providerRequired = isMissingRequiredDropdownField(
    normalizedConfig?.provider,
    provider,
  );
  const inheritedModelLabel =
    inheritedModelDefault.source === "build"
      ? getBakedModelInheritLabel(inheritedModelDefault.value)
      : getDefaultLlmModelLabel(inheritedModelDefault.value);
  const {
    isRelayMesh,
    options: effectiveModelOptions,
    selectValue: modelSelectValue,
    showCustomInput: showCustomModelInput,
  } = relayMeshModelPickerState({
    discoveredOptions: discoveredModelOptions,
    fallbackOptions: [{ id: "", label: inheritedModelLabel }],
    isCustomEditing: isCustomModelEditing,
    model,
    provider: providerForDiscovery,
  });
  const modelDropdownOptions = buildModelDropdownOptions({
    allowCustom: !isRelayMesh,
    globalModel: isRelayMesh ? undefined : inheritedModelDefault.value,
    globalModelLabel: isRelayMesh ? undefined : inheritedModelLabel,
    loading: modelDiscoveryLoading && discoveredModelOptions === null,
    loadingValue: MODEL_DISCOVERY_LOADING_VALUE,
    options: effectiveModelOptions,
  });
  const modelStatusMessage = resolveModelFieldStatusMessage({
    discoveredModelOptions,
    loading: modelDiscoveryLoading,
    status: modelDiscoveryStatus,
  });

  // Provider field derived state
  const trimmedProvider = provider.trim();
  const hideProviderIds = buildHiddenProviderIds(bakedEnvKeys);
  const providerOptions = getPersonaProviderOptions(
    trimmedProvider,
    selectedRuntime?.id ?? "",
    inheritedProviderDefault.source === "global"
      ? inheritedProviderDefault.value
      : "",
    hideProviderIds,
    selectedRuntime?.supportsChatgptSubscription,
  );
  const providerSelectValue = isCustomProviderEditing
    ? CUSTOM_PROVIDER_DROPDOWN_VALUE
    : trimmedProvider || AUTO_PROVIDER_DROPDOWN_VALUE;
  const providerDropdownOptions: PersonaDropdownOption[] = [
    ...providerOptions.map((option) => ({
      label:
        option.id === "" && inheritedProviderDefault.source === "build"
          ? getBakedProviderInheritLabel(
              inheritedProviderDefault.value,
              providerOptions,
            )
          : option.label,
      value: option.id || AUTO_PROVIDER_DROPDOWN_VALUE,
    })),
    { label: "Custom provider...", value: CUSTOM_PROVIDER_DROPDOWN_VALUE },
  ];

  const previewLabel = name.trim() || "Agent name";
  const previewAvatarUrl = avatarUrl.trim() || null;
  const advancedFieldsTransition = shouldReduceMotion
    ? { duration: 0 }
    : ADVANCED_FIELDS_MOTION_TRANSITION;
  // Displayed inline when either the locked update or a standalone setter fails.
  // setterError takes precedence — the update already committed when it fires.
  const displayError =
    setterError ??
    (updateMutation.error instanceof Error ? updateMutation.error : null);

  return (
    <Dialog onOpenChange={handleOpenChange} open={open}>
      <ChooserDialogContent
        className="max-w-3xl border-0"
        contentClassName="pt-3"
        data-testid="edit-agent-dialog"
        footerClassName="border-t-0 pt-0"
        headerClassName="pb-2"
        title={`Edit ${agent.name}`}
        footer={
          <div className="flex w-full items-center justify-end gap-2">
            <Button
              disabled={isSaving || isAvatarUploadPending}
              onClick={() => handleOpenChange(false)}
              type="button"
              variant="outline"
            >
              Cancel
            </Button>
            <Button
              data-testid="edit-agent-dialog-submit"
              disabled={!canSubmit}
              onClick={() => void handleSubmit()}
              type="button"
            >
              {isSaving ? "Saving..." : "Save changes"}
            </Button>
          </div>
        }
      >
        <div className="grid gap-5 lg:grid-cols-[220px_minmax(0,1fr)]">
          {/* Avatar is definition-level identity. hideEditControl suppresses
              the internal pencil badge; the CTA below is the only edit path. */}
          <div className="flex flex-col items-center gap-2">
            <AgentCreationPreview
              avatarUrl={previewAvatarUrl}
              hideEditControl
              label={previewLabel}
              onClearAvatar={() => setAvatarUrl("")}
              onUploadPendingChange={setIsAvatarUploadPending}
              onSelectAvatar={setAvatarUrl}
            />
          </div>
          <div className="space-y-5">
            <div className="space-y-1.5">
              <label
                className="text-sm font-medium text-foreground"
                htmlFor="edit-agent-name"
              >
                Agent name
              </label>
              <div
                className={cn(
                  "flex min-h-11 items-center px-3",
                  PERSONA_FIELD_SHELL_CLASS,
                )}
              >
                <Input
                  autoCorrect="off"
                  className={cn(
                    "h-8 px-0 py-0 leading-6",
                    PERSONA_FIELD_CONTROL_CLASS,
                  )}
                  disabled={isSaving}
                  id="edit-agent-name"
                  onChange={(event) => setName(event.target.value)}
                  placeholder="Agent name"
                  value={name}
                />
              </div>
            </div>
            <div className="space-y-1.5">
              <label
                className="text-sm font-medium text-foreground"
                htmlFor="edit-agent-description"
              >
                Description
              </label>
              <div
                className={cn(
                  "flex min-h-11 items-center px-3",
                  PERSONA_FIELD_SHELL_CLASS,
                )}
              >
                <Input
                  className={cn(
                    "h-8 px-0 py-0 leading-6",
                    PERSONA_FIELD_CONTROL_CLASS,
                  )}
                  disabled={isSaving}
                  id="edit-agent-description"
                  onChange={(event) => setDescription(event.target.value)}
                  placeholder="What this agent does"
                  value={description}
                />
              </div>
            </div>
            <OwnerOnlyAccessField
              accessLocked={agentAccessOwnerOnly === true}
              allowlist={respondToAllowlist}
              disabled={isSaving}
              mode={respondTo}
              onAllowlistChange={setRespondToAllowlist}
              onModeChange={setRespondTo}
            />
            <RunOnSummarySection backend={agent.backend} />

            {/* Provider (runtime) */}
            <div className="space-y-1.5">
              <label
                className="text-sm font-medium text-foreground"
                htmlFor="edit-agent-runtime"
              >
                Provider
              </label>
              <PersonaDropdownField
                disabled={isSaving}
                id="edit-agent-runtime"
                onValueChange={handleRuntimeDropdownChange}
                options={runtimeDropdownOptions}
                placeholder="Choose a provider"
                value={runtimeDropdownValue}
              />
              {selectedRuntime ? (
                <p className="text-xs text-muted-foreground">
                  Detected at{" "}
                  <span className="font-medium">
                    {selectedRuntime.binaryPath ??
                      selectedRuntime.command ??
                      selectedRuntime.id}
                  </span>
                </p>
              ) : null}
              <AddCustomHarnessDialog
                onOpenChange={setIsAddHarnessOpen}
                onSaved={selectSavedHarness}
                open={isAddHarnessOpen}
              />
            </div>
            {selectedRuntimeId === "custom" ? (
              <div className="space-y-1.5">
                <label
                  className="text-sm font-medium text-foreground"
                  htmlFor="edit-agent-command"
                >
                  Agent command
                </label>
                <div
                  className={cn(
                    "flex min-h-11 items-center px-3",
                    PERSONA_FIELD_SHELL_CLASS,
                  )}
                >
                  <Input
                    autoCorrect="off"
                    className={cn(
                      "h-8 px-0 py-0 leading-6",
                      PERSONA_FIELD_CONTROL_CLASS,
                    )}
                    disabled={isSaving}
                    id="edit-agent-command"
                    onChange={(event) => setAgentCommand(event.target.value)}
                    placeholder="Full path or shell command"
                    value={agentCommand}
                  />
                </div>
              </div>
            ) : null}
            {/* LLM provider + provider API key + model (+ effort companion) */}
            <EditAgentProviderModelFields
              disabled={isSaving}
              effort={{
                backend: agent.backend,
                // Post-switch: options from the prospective runtime's catalog
                // fallback, value from the stored column (see launchEffortLevel).
                config: runtimeTouched.current
                  ? undefined
                  : configSurfaceQuery.data,
                onChange: (level) => {
                  effortTouched.current = true;
                  setEffortLevel(level);
                },
                runtime: prospectiveRuntime,
                value: effortTouched.current ? effortLevel : launchEffortLevel,
              }}
              llmProviderFieldVisible={llmProviderFieldVisible}
              harnessProviderChoice={harnessProviderChoice}
              providerRequired={providerRequired}
              providerDropdownOptions={providerDropdownOptions}
              providerSelectValue={providerSelectValue}
              onProviderDropdownChange={handleProviderDropdownChange}
              isCustomProviderEditing={isCustomProviderEditing}
              provider={provider}
              onProviderChange={setProvider}
              topLevelSecretEnvVar={topLevelSecretEnvVar}
              apiKeyIsInherited={apiKeyIsInherited}
              apiKeyInheritedLabel={apiKeyInheritedLabel}
              apiKeyIsRequired={apiKeyIsRequired}
              effectiveProvider={effectiveProvider}
              apiKeyValue={apiKeyValue}
              onApiKeyChange={(next) => {
                setEnvVars((prev) => ({
                  ...prev,
                  [topLevelSecretEnvVar as string]: next,
                }));
              }}
              modelRequired={modelRequired}
              modelDiscoveryLoading={modelDiscoveryLoading}
              modelDropdownOptions={modelDropdownOptions}
              modelSelectValue={modelSelectValue}
              onModelDropdownChange={handleModelDropdownChange}
              showCustomModelInput={showCustomModelInput}
              model={model}
              onModelChange={setModel}
              modelStatusMessage={modelStatusMessage}
            />

            {/* Kuzz effort sits with the model choice, like the ACP picker. */}
            {isKuzzAgentRuntime(prospectiveRuntimeId) ? (
              <KuzzAgentModelTuningFields
                catalogEfforts={discoveredModelOptions?.find(
                  (option) => option.id === (submission.model ?? ""),
                )}
                disabled={isSaving}
                envVars={envVars}
                inheritedEnvVars={inheritedEnvVarsForAdvanced}
                model={submission.model ?? ""}
                onEnvVarChange={(key, value) => {
                  setEnvVars((prev) => {
                    const next = { ...prev };
                    if (value === "") {
                      delete next[key];
                    } else {
                      next[key] = value;
                    }
                    return next;
                  });
                }}
                provider={effectiveProvider}
              />
            ) : null}

            <AgentAiDefaultsNotice
              onEditDefaults={() => {
                if (!isSaving) setAiDefaultsOpen(true);
              }}
              triggerRef={aiDefaultsTriggerRef}
              explicitModel={submission.model ?? ""}
              explicitProvider={submission.provider ?? ""}
              inheritedModel={inheritedModelDefault}
              inheritedProvider={inheritedProviderDefault}
            />
            <AgentDefaultsDialog
              onOpenChange={setAiDefaultsOpen}
              open={aiDefaultsOpen}
              returnFocusRef={aiDefaultsTriggerRef}
            />

            {/* Advanced settings */}
            <div className="space-y-3">
              <button
                aria-expanded={showAdvancedFields}
                className="inline-flex h-9 items-center gap-1.5 text-sm font-medium text-foreground transition-colors hover:text-foreground/80 focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50"
                disabled={isSaving}
                onClick={() => setShowAdvancedFields((current) => !current)}
                type="button"
              >
                <span>Advanced</span>
                <AdvancedRequiredBadge
                  envVars={submission.envVars}
                  requiredEnvKeys={advancedRequiredEnvKeys}
                  testId="edit-agent-advanced-required-badge"
                />
                <ChevronDown
                  className={cn(
                    "h-4 w-4 text-muted-foreground transition-transform duration-150 ease-out",
                    showAdvancedFields && "rotate-180",
                  )}
                />
              </button>
              <AnimatePresence initial={false}>
                {showAdvancedFields ? (
                  <motion.div
                    animate={{ height: "auto", opacity: 1, scale: 1 }}
                    className="origin-top overflow-hidden"
                    exit={{ height: 0, opacity: 0, scale: 0.98 }}
                    initial={{ height: 0, opacity: 0, scale: 0.98 }}
                    key="edit-agent-advanced-fields"
                    transition={advancedFieldsTransition}
                  >
                    <EditAgentAdvancedFields
                      acpCommand={acpCommand}
                      acpCommandCandidates={acpCommandsQuery.data ?? []}
                      agentArgs={agentArgs}
                      autoRestartOnConfigChange={autoRestartOnConfigChange}
                      disabled={isSaving}
                      envVars={envVars}
                      fileSatisfiedEnvKeys={fileSatisfiedEnvKeys}
                      hiddenEnvKeys={
                        topLevelSecretEnvVar ? [topLevelSecretEnvVar] : []
                      }
                      focusKey={
                        initialFocus?.type === "env_key"
                          ? initialFocus.key
                          : undefined
                      }
                      inheritedEnvVars={inheritedEnvVarsForAdvanced}
                      modelTuningRuntimeId={prospectiveRuntimeId}
                      parallelism={parallelism}
                      requiredEnvKeys={advancedRequiredEnvKeys}
                      catalogStatus={runtimeCatalogStatus}
                      selectedRuntime={prospectiveRuntime}
                      systemPrompt={systemPrompt}
                      onAcpCommandChange={setAcpCommand}
                      onAgentArgsChange={setAgentArgs}
                      onAutoRestartChange={setAutoRestartOnConfigChange}
                      onEnvVarsChange={setEnvVars}
                      onParallelismChange={setParallelism}
                      onSystemPromptChange={setSystemPrompt}
                    />
                  </motion.div>
                ) : null}
              </AnimatePresence>
            </div>

            {/* Error — covers both the locked update (React Query) and the
                standalone setters (setterError); setter error takes precedence
                since the update already committed when it fires. */}
            {displayError != null ? (
              <p className="text-sm text-destructive">{displayError.message}</p>
            ) : null}
          </div>
        </div>
      </ChooserDialogContent>
    </Dialog>
  );
}
