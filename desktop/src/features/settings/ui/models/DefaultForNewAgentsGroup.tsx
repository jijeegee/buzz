import * as React from "react";

import { SettingsOptionGroup, SettingsOptionRow } from "../SettingsOptionGroup";
import type { DefaultOption } from "./providersSettingsModel";
import { SaveNoticeLine, type SaveNotice } from "./SaveNoticeLine";

/**
 * The one "Default for new agents" radio group across every provider × path.
 * `GlobalAgentConfig.preferred_runtime` / `provider` is a single value, so a
 * per-provider control would read as several defaults at once; one group
 * keeps exactly one option selected. Picking an option is one save.
 */
export function DefaultForNewAgentsGroup({
  disabled,
  notice,
  onChoose,
  options,
}: {
  disabled: boolean;
  notice: SaveNotice | null;
  onChoose: (option: DefaultOption) => void;
  options: readonly DefaultOption[];
}) {
  const titleId = React.useId();
  const groupName = React.useId();

  return (
    <SettingsOptionGroup
      data-testid="settings-models-default"
      description="New agents start on this provider and sign-in path unless their own settings say otherwise. The default model and effort stay in Agent defaults."
      title={<span id={titleId}>Default for new agents</span>}
    >
      {options.length === 0 ? (
        <SettingsOptionRow data-testid="settings-models-default-empty">
          <p className="text-sm text-muted-foreground/70" data-settings-subcopy>
            Install an agent runtime under Settings › Agents to choose a
            default.
          </p>
        </SettingsOptionRow>
      ) : (
        // The group holds radios and their labels only; status lines live
        // outside it so screen readers count the options, not the notices.
        <div
          aria-labelledby={titleId}
          className="divide-y divide-border/55"
          role="radiogroup"
        >
          {options.map((option) => {
            const inputId = `${groupName}-${option.id}`;
            const locked = disabled || (!option.available && !option.selected);
            return (
              <SettingsOptionRow
                data-testid={`settings-models-default-${option.id}`}
                key={option.id}
              >
                <label
                  className={
                    locked
                      ? "min-w-0 flex-1 opacity-60"
                      : "min-w-0 flex-1 cursor-pointer"
                  }
                  htmlFor={inputId}
                >
                  <span className="block font-medium text-foreground">
                    {option.label}
                  </span>
                  <span
                    className="mt-0.5 block text-sm text-muted-foreground/70"
                    data-settings-subcopy
                  >
                    {option.detail}
                  </span>
                </label>
                <input
                  checked={option.selected}
                  className="size-4 shrink-0 accent-primary disabled:cursor-not-allowed"
                  disabled={locked}
                  id={inputId}
                  name={groupName}
                  onChange={() => onChoose(option)}
                  type="radio"
                  value={option.id}
                />
              </SettingsOptionRow>
            );
          })}
        </div>
      )}
      {notice ? (
        <div className="px-4 py-3">
          <SaveNoticeLine notice={notice} />
        </div>
      ) : null}
    </SettingsOptionGroup>
  );
}
