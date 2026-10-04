import * as React from "react";

import { getAgentWorkingState } from "@/features/agents/agentWorkingSignal";
import {
  useChannelRoutingQuery,
  useSetChannelRoutingMutation,
} from "@/features/agents/channelRoutingHooks";
import {
  hostPickerOptions,
  modeNeedsAgent,
  ROUTING_MODE_OPTIONS,
  routingAgentLabel,
  routingNameOf,
  routingRestartAffordances,
  routingStatusLine,
} from "@/features/agents/lib/channelRouting";
import type { ChannelRoutingMode } from "@/shared/api/tauriChannelRouting";
import type { ManagedAgent } from "@/shared/api/types";
import { cn } from "@/shared/lib/cn";
import { ChannelRoutingAgentPicker } from "./ChannelRoutingAgentPicker";
import { ChannelRoutingStatusLine } from "./ChannelRoutingStatusLine";

/**
 * Agents page › "Channel routing": the one place the routing mode and the
 * routing agent (the starred record) are edited. Rust owns both and the
 * transition plan; this card reads `get_channel_routing` and writes through
 * one `set_channel_routing(mode, agent)` per user action.
 *
 * Choosing Host with no routing agent yet only opens the picker (a draft) —
 * nothing is saved until an agent is picked, so the radio alone never leaves
 * a half-configured mode on disk.
 */
export function ChannelRoutingCard({
  agents,
  onRestartAgent,
  restartingAgentPubkey,
}: {
  agents: readonly ManagedAgent[];
  onRestartAgent: (pubkey: string) => void;
  restartingAgentPubkey: string | null;
}) {
  const titleId = React.useId();
  const descriptionId = React.useId();
  const groupName = React.useId();
  const statusQuery = useChannelRoutingQuery();
  const { mutate: saveRouting, ...saveMutation } =
    useSetChannelRoutingMutation();
  const [draftMode, setDraftMode] = React.useState<ChannelRoutingMode | null>(
    null,
  );

  const status = statusQuery.data;
  const pickerOptions = React.useMemo(
    () => hostPickerOptions(agents),
    [agents],
  );
  const nameOf = React.useMemo(() => routingNameOf(agents), [agents]);
  const selectedMode = draftMode ?? status?.mode ?? null;
  const isSaving = saveMutation.isPending;

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
    const remembered =
      status.routingAgent !== null &&
      pickerOptions.some((option) => option.pubkey === status.routingAgent)
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
              const locked = isSaving || !option.available;
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
                        option.available
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
                  {checked &&
                  modeNeedsAgent(option.mode) &&
                  option.available ? (
                    <>
                      <ChannelRoutingAgentPicker
                        disabled={isSaving}
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

          {saveMutation.error ? (
            <p className="mt-2 text-sm text-destructive" role="alert">
              {saveMutation.error instanceof Error
                ? saveMutation.error.message
                : String(saveMutation.error)}
            </p>
          ) : null}

          <div className="mt-3">
            <ChannelRoutingStatusLine
              affordances={routingRestartAffordances(status, {
                isWorking: (pubkey) =>
                  getAgentWorkingState(pubkey).source !== "none",
                nameOf,
              })}
              isRestarting={(pubkey) => restartingAgentPubkey === pubkey}
              line={routingStatusLine(status, nameOf)}
              onRestart={onRestartAgent}
            />
          </div>
        </>
      ) : null}
    </section>
  );
}
