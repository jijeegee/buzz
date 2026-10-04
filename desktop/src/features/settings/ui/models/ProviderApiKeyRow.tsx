import * as React from "react";

import { PersonaProviderApiKeyField } from "@/features/agents/ui/PersonaProviderApiKeyField";
import { Button } from "@/shared/ui/button";

import type { ProviderRow } from "./providersSettingsModel";
import { SaveNoticeLine, type SaveNotice } from "./SaveNoticeLine";

/**
 * The API-key path of one provider: the shared `PersonaProviderApiKeyField`
 * as a pure draft over `GlobalAgentConfig.env_vars[apiKeyEnvVar]`, with
 * Save (one write) and Remove key (one write with an empty value). The saved
 * secret is never echoed back; a set key shows as a placeholder.
 */
export function ProviderApiKeyRow({
  companions,
  disabled,
  notice,
  onRemove,
  onSave,
  row,
}: {
  /** Providers that read the same key, named in the row's footnote. */
  companions: readonly ProviderRow[];
  disabled: boolean;
  notice: SaveNotice | null;
  onRemove: () => Promise<boolean>;
  /** Resolves true when the key was persisted so the draft can clear. */
  onSave: (value: string) => Promise<boolean>;
  row: ProviderRow;
}) {
  const [draft, setDraft] = React.useState("");
  if (row.apiKeyEnvVar === null || row.apiKeyLabel === null) return null;
  const envVar = row.apiKeyEnvVar;
  const canSave = draft.trim().length > 0 && !disabled;

  async function handleSave() {
    if (!canSave) return;
    if (await onSave(draft)) setDraft("");
  }

  return (
    <div
      className="space-y-3 px-4 py-3.5 text-sm"
      data-testid={`settings-models-${row.providerId}-api-key`}
    >
      <p className="text-xs font-medium text-muted-foreground/70">API key</p>
      <PersonaProviderApiKeyField
        disabled={disabled}
        envVarName={envVar}
        inheritedLabel="Saved on this device — paste a new key to replace it"
        isInherited={row.apiKeyIsSet}
        isRequired={false}
        label={row.apiKeyLabel}
        onValueChange={setDraft}
        value={draft}
      />
      {companions.length > 0 ? (
        <p className="text-xs text-muted-foreground/70" data-settings-subcopy>
          Also used by{" "}
          {companions.map((companion) => companion.label).join(", ")} agents.
        </p>
      ) : null}
      <div className="flex flex-wrap items-center gap-3">
        <SaveNoticeLine notice={notice} />
        <div className="ml-auto flex items-center gap-2">
          {row.apiKeyIsSet ? (
            <Button
              data-testid={`settings-models-${row.providerId}-remove-key`}
              disabled={disabled}
              onClick={() => void onRemove()}
              size="sm"
              type="button"
              variant="outline"
            >
              Remove key
            </Button>
          ) : null}
          <Button
            data-testid={`settings-models-${row.providerId}-save-key`}
            disabled={!canSave}
            onClick={() => void handleSave()}
            size="sm"
            type="button"
          >
            Save
          </Button>
        </div>
      </div>
    </div>
  );
}
