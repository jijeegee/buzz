import * as React from "react";

import type { RoutingAgentOption } from "@/features/agents/lib/channelRouting";
import {
  setDefaultAiAutoJoin,
  useDefaultAiAutoJoin,
} from "@/features/agents/lib/defaultAiPreferences";
import { Checkbox } from "@/shared/ui/checkbox";

/**
 * The routing-agent picker that unfolds under the selected Host (later Lead)
 * radio, plus the "Join new channels I create" preference that only means
 * something while a routing agent exists. Picking an agent is the save: the
 * card turns it into one `set_channel_routing(mode, agent)`.
 *
 * One label owner each: the select is named by its `<label htmlFor>`, the
 * checkbox by its own.
 */
export function ChannelRoutingAgentPicker({
  disabled,
  label,
  onChoose,
  options,
  selectedPubkey,
}: {
  disabled: boolean;
  /** "Host agent" / "Lead agent". */
  label: string;
  onChoose: (pubkey: string) => void;
  options: readonly RoutingAgentOption[];
  /** The saved routing agent, or null while none is chosen for this mode. */
  selectedPubkey: string | null;
}) {
  const selectId = React.useId();
  const autoJoinId = React.useId();
  const autoJoin = useDefaultAiAutoJoin();
  const value =
    selectedPubkey && options.some((option) => option.pubkey === selectedPubkey)
      ? selectedPubkey
      : "";

  return (
    <div
      className="ml-7 mt-2 space-y-2"
      data-testid="agents-channel-routing-picker"
    >
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <label
          className="w-28 shrink-0 text-sm font-medium text-foreground"
          htmlFor={selectId}
        >
          {label}
        </label>
        {options.length === 0 ? (
          <p
            className="text-sm text-muted-foreground"
            data-testid="agents-channel-routing-no-agents"
          >
            No agents yet — create one below.
          </p>
        ) : (
          <select
            className="flex h-9 min-w-0 max-w-xs flex-1 rounded-md border border-input bg-background px-3 py-2 text-sm shadow-xs disabled:cursor-not-allowed disabled:opacity-60"
            data-testid="agents-channel-routing-agent-select"
            disabled={disabled}
            id={selectId}
            onChange={(event) => {
              if (event.target.value) onChoose(event.target.value);
            }}
            value={value}
          >
            {value === "" ? (
              <option disabled value="">
                Choose an agent…
              </option>
            ) : null}
            {options.map((option) => (
              <option key={option.pubkey} value={option.pubkey}>
                {option.name}
              </option>
            ))}
          </select>
        )}
      </div>
      <div className="flex items-center gap-2 sm:pl-[7.75rem]">
        <Checkbox
          checked={autoJoin}
          data-testid="agents-channel-routing-auto-join"
          id={autoJoinId}
          onCheckedChange={(checked) => setDefaultAiAutoJoin(checked === true)}
        />
        <label className="text-sm text-foreground" htmlFor={autoJoinId}>
          Join new channels I create
        </label>
      </div>
    </div>
  );
}
