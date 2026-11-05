import * as React from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";

import { useAgentAccessOwnerOnlyQuery } from "@/features/agents/useAgentAccessOwnerOnly";
import {
  CreateAgentRespondToField,
  ownerOnlyAccessDisabledReason,
} from "@/features/agents/ui/RespondToField";
import {
  getExternalAgentAccess,
  setExternalAgentAccess,
} from "@/shared/api/agentPairing";
import type { RespondToMode } from "@/shared/api/types";
import { useT } from "@/shared/i18n";
import { Button } from "@/shared/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui/dialog";

/** Who a paired agent answers; the agent's own connector applies it live. */
export function EditExternalAccessDialog({
  agent,
  currentPubkey,
  onOpenChange,
}: {
  agent: { pubkey: string; name: string };
  currentPubkey: string;
  onOpenChange: (open: boolean) => void;
}) {
  const t = useT();
  const queryClient = useQueryClient();
  const queryKey = ["external-agent-access", currentPubkey, agent.pubkey];
  const access = useQuery({
    queryKey,
    queryFn: () => getExternalAgentAccess(agent.pubkey),
    gcTime: 0,
  });
  const { data: agentAccessOwnerOnly } = useAgentAccessOwnerOnlyQuery();
  const accessLocked = agentAccessOwnerOnly === true;
  const [respondTo, setRespondTo] = React.useState<RespondToMode>("owner-only");
  const [allowlist, setAllowlist] = React.useState<string[]>([]);
  const [saving, setSaving] = React.useState(false);
  const [saveError, setSaveError] = React.useState<string | null>(null);

  React.useEffect(() => {
    if (access.data) {
      setRespondTo(access.data.respondTo);
      setAllowlist([...access.data.respondToAllowlist]);
    }
  }, [access.data]);

  const valid = respondTo !== "allowlist" || allowlist.length > 0;
  const disabled = !access.data || saving || accessLocked;
  const error =
    saveError ?? (access.error instanceof Error ? access.error.message : null);

  async function save() {
    setSaving(true);
    setSaveError(null);
    try {
      await setExternalAgentAccess(
        agent.pubkey,
        agent.name,
        respondTo,
        respondTo === "allowlist" ? allowlist : [],
      );
      void queryClient.invalidateQueries({ queryKey });
      onOpenChange(false);
    } catch (failure) {
      setSaveError(
        failure instanceof Error ? failure.message : String(failure),
      );
    } finally {
      setSaving(false);
    }
  }

  return (
    <Dialog onOpenChange={onOpenChange} open>
      <DialogContent
        className="max-w-md"
        data-testid="external-agent-access-dialog"
      >
        <DialogHeader>
          <DialogTitle>{t("channels.respondTo.title")}</DialogTitle>
          <DialogDescription>
            {t("channels.respondTo.bodyNamed", { name: agent.name })}
          </DialogDescription>
        </DialogHeader>
        <CreateAgentRespondToField
          allowlist={accessLocked ? [] : allowlist}
          disabled={disabled}
          disabledReason={
            accessLocked ? ownerOnlyAccessDisabledReason() : undefined
          }
          mode={accessLocked ? "owner-only" : respondTo}
          onAllowlistChange={setAllowlist}
          onModeChange={setRespondTo}
          ownerPubkey={currentPubkey}
        />
        {error ? <p className="text-sm text-destructive">{error}</p> : null}
        <div className="flex justify-end gap-2">
          <Button
            onClick={() => onOpenChange(false)}
            size="sm"
            type="button"
            variant="outline"
          >
            {t("common.cancel")}
          </Button>
          <Button
            data-testid="external-agent-access-save"
            disabled={disabled || !valid}
            onClick={() => void save()}
            size="sm"
            type="button"
          >
            {saving ? t("channels.edit.saving") : t("channels.respondTo.save")}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
