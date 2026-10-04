import * as React from "react";

import {
  getAgentWorkingState,
  subscribeAgentWorkingSignal,
} from "@/features/agents/agentWorkingSignal";
import {
  useChannelRoutingQuery,
  useCreateHostAgentMutation,
  useSetChannelRoutingMutation,
} from "@/features/agents/channelRoutingHooks";
import { HOST_PERSONA_ID } from "@/features/agents/lib/hostAgent";
import {
  modeNeedsAgent,
  ROUTING_MODE_OPTIONS,
  routingAgentLabel,
  routingNameOf,
  routingPickerOptions,
  routingRestartAffordances,
  routingStatusLine,
} from "@/features/agents/lib/channelRouting";
import { requestModelsSettingsTab } from "@/features/settings/lib/modelsSettingsTabRequest";
import { useMessageRoutingModelQuery } from "@/features/settings/taskModelsHooks";
import type { ChannelRoutingMode } from "@/shared/api/tauriChannelRouting";
import type { ManagedAgent } from "@/shared/api/types";
import { cn } from "@/shared/lib/cn";
import { Button } from "@/shared/ui/button";
import { ChannelRoutingAgentPicker } from "./ChannelRoutingAgentPicker";
import { ChannelRoutingSmartModelLine } from "./ChannelRoutingSmartModelLine";
import { ChannelRoutingStatusLine } from "./ChannelRoutingStatusLine";

/**
 * Agents page › "Channel routing": the one place the routing mode and the
 * routing agent (the starred record) are edited. Rust owns both and the
 * transition plan; this card reads `get_channel_routing` and writes through
 * one `set_channel_routing(mode, agent)` per user action.
 *
 * Choosing Host or Lead with no eligible routing agent yet only opens the
 * picker (a draft) —
 * nothing is saved until an agent is picked, so the radio alone never leaves
 * a half-configured mode on disk. Until some agent is an instance of the
 * built-in Host persona, Host offers "Create a Host agent": create one,
 * then save it as the routing agent (two writes; the first alone leaves an
 * ordinary agent, which is a valid state).
 *
 * Smart routing is selectable only while its router model is ready (an API
 * key in Settings › Models); the line under it names the model or links to
 * where the key goes.
 */
export function ChannelRoutingCard({
  agents,
  onOpenModelsSettings,
  onRestartAgent,
  restartingAgentPubkey,
}: {
  agents: readonly ManagedAgent[];
  /** Navigate to Settings › Models (the tab is requested before calling). */
  onOpenModelsSettings: () => void;
  onRestartAgent: (pubkey: string) => void;
  restartingAgentPubkey: string | null;
}) {
  const titleId = React.useId();
  const descriptionId = React.useId();
  const groupName = React.useId();
  const statusQuery = useChannelRoutingQuery();
  const routerModel = useMessageRoutingModelQuery();
  const { mutate: saveRouting, ...saveMutation } =
    useSetChannelRoutingMutation();
  const { mutate: createHostAgent, ...createHostMutation } =
    useCreateHostAgentMutation();
  const [draftMode, setDraftMode] = React.useState<ChannelRoutingMode | null>(
    null,
  );

  const status = statusQuery.data;
  // Subscribe to the working signal for the agents a restart could touch, so
  // "Restart now" disables the moment one starts a turn. The snapshot is a
  // string so it is stable between unrelated signal changes.
  const workingStale = React.useSyncExternalStore(
    subscribeAgentWorkingSignal,
    () =>
      (status?.agents ?? [])
        .filter(
          (agent) =>
            agent.stale && getAgentWorkingState(agent.pubkey).source !== "none",
        )
        .map((agent) => agent.pubkey)
        .join(","),
  );
  const isWorking = (pubkey: string) =>
    workingStale.split(",").includes(pubkey);
  // Re-check at click time too: the signal can move between render and click.
  const restartIfIdle = (pubkey: string) => {
    if (getAgentWorkingState(pubkey).source !== "none") return;
    onRestartAgent(pubkey);
  };
  const nameOf = React.useMemo(() => routingNameOf(agents), [agents]);
  // An existing Host instance is already in the picker; offer no second one.
  const canCreateHost = !agents.some(
    (agent) => agent.personaId === HOST_PERSONA_ID,
  );
  const selectedMode = draftMode ?? status?.mode ?? null;
  const pickerOptions = React.useMemo(
    () => (selectedMode ? routingPickerOptions(selectedMode, agents) : []),
    [selectedMode, agents],
  );
  const isSaving = saveMutation.isPending || createHostMutation.isPending;
  const actionError = saveMutation.error ?? createHostMutation.error;

  const save = (mode: ChannelRoutingMode, agentPubkey: string | null) => {
    saveRouting({ mode, agentPubkey }, { onSuccess: () => setDraftMode(null) });
  };

  const chooseMode = (mode: ChannelRoutingMode) => {
    if (!status) return;
    if (!modeNeedsAgent(mode)) {
      setDraftMode(null);
      if (mode !== status.mode) save(mode, null);
      return;
    }
    // The remembered agent carries over only if it can take this mode's role
    // (a remote host can't lead).
    const remembered =
      status.routingAgent !== null &&
      routingPickerOptions(mode, agents).some(
        (option) =>
          option.pubkey === status.routingAgent &&
          option.disabledReason === null,
      )
        ? status.routingAgent
        : null;
    if (remembered === null) {
      // No agent to route with yet: open the picker, save on the pick.
      setDraftMode(mode);
      return;
    }
    setDraftMode(null);
    if (mode !== status.mode) save(mode, remembered);
  };

  return (
    // The heading names the radio group (below), not the section, so the
    // label has one owner and screen readers hit it once.
    <section
      className="rounded-2xl border border-border/70 bg-card/40 p-4 sm:p-5"
      data-testid="agents-channel-routing"
    >
      <h2 className="text-base font-semibold text-foreground" id={titleId}>
        Channel routing
      </h2>
      <p className="mt-0.5 text-sm text-muted-foreground" id={descriptionId}>
        How agents pick up messages nobody @mentioned.
      </p>

      {statusQuery.isError ? (
        <p className="mt-3 text-sm text-destructive" role="alert">
          Couldn't load channel routing.
        </p>
      ) : null}

      {status ? (
        <>
          <fieldset
            aria-describedby={descriptionId}
            aria-labelledby={titleId}
            className="mt-3 space-y-1"
            data-testid="agents-channel-routing-modes"
          >
            {ROUTING_MODE_OPTIONS.map((option) => {
              const inputId = `${groupName}-${option.mode}`;
              const checked = selectedMode === option.mode;
              const isSmartRouting = option.mode === "desktop-router";
              const selectable =
                option.available &&
                (!isSmartRouting || routerModel.status?.ready === true);
              const locked = isSaving || !selectable;
              return (
                <div
                  data-testid={`agents-channel-routing-mode-${option.mode}`}
                  key={option.mode}
                >
                  <div className="flex items-start gap-3 rounded-lg px-1 py-1.5">
                    <input
                      checked={checked}
                      className="mt-1 size-4 shrink-0 accent-primary disabled:cursor-not-allowed"
                      disabled={locked}
                      id={inputId}
                      name={groupName}
                      onChange={() => chooseMode(option.mode)}
                      type="radio"
                      value={option.mode}
                    />
                    <label
                      className={cn(
                        "min-w-0 flex-1 sm:flex sm:gap-3",
                        selectable
                          ? "cursor-pointer"
                          : "cursor-not-allowed opacity-60",
                      )}
                      htmlFor={inputId}
                    >
                      <span className="block w-28 shrink-0 text-sm font-medium text-foreground">
                        {option.label}
                      </span>
                      <span className="block text-sm text-muted-foreground">
                        {option.description}
                        {option.available ? null : " Coming soon."}
                      </span>
                    </label>
                  </div>
                  {isSmartRouting && option.available ? (
                    <ChannelRoutingSmartModelLine
                      onOpenModels={(tab) => {
                        requestModelsSettingsTab(tab);
                        onOpenModelsSettings();
                      }}
                      status={routerModel.status}
                      statusError={routerModel.isError}
                    />
                  ) : null}
                  {checked &&
                  modeNeedsAgent(option.mode) &&
                  option.available ? (
                    <>
                      <ChannelRoutingAgentPicker
                        disabled={isSaving}
                        createAction={
                          option.mode === "host" && canCreateHost ? (
                            <Button
                              data-testid="agents-channel-routing-create-host"
                              disabled={isSaving}
                              onClick={() =>
                                createHostAgent(undefined, {
                                  onSuccess: (pubkey) => save("host", pubkey),
                                })
                              }
                              size="sm"
                              type="button"
                              variant="outline"
                            >
                              {createHostMutation.isPending
                                ? "Creating…"
                                : "Create a Host agent"}
                            </Button>
                          ) : undefined
                        }
                        label={routingAgentLabel(option.mode)}
                        onChoose={(pubkey) => save(option.mode, pubkey)}
                        options={pickerOptions}
                        selectedPubkey={
                          draftMode === option.mode ? null : status.routingAgent
                        }
                      />
                      {draftMode === option.mode ? (
                        <p
                          className="ml-7 mt-2 text-sm text-amber-700 dark:text-amber-400"
                          data-testid="agents-channel-routing-draft-hint"
                        >
                          Choose a {option.label.toLowerCase()} agent to turn
                          this on. Nothing changes until you do.
                        </p>
                      ) : null}
                    </>
                  ) : null}
                </div>
              );
            })}
          </fieldset>

          {actionError ? (
            <p className="mt-2 text-sm text-destructive" role="alert">
              {actionError instanceof Error
                ? actionError.message
                : String(actionError)}
            </p>
          ) : null}

          <div className="mt-3">
            <ChannelRoutingStatusLine
              affordances={routingRestartAffordances(status, {
                isWorking,
                nameOf,
              })}
              isRestarting={(pubkey) => restartingAgentPubkey === pubkey}
              line={routingStatusLine(status, nameOf)}
              onRestart={restartIfIdle}
            />
          </div>
        </>
      ) : null}
    </section>
  );
}
