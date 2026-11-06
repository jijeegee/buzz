import * as React from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";

import { clampAgentDescription } from "@/features/agents/lib/agentDescription";
import { AgentCreationPreview } from "@/features/agents/ui/AgentCreationPreview";
import { AgentIdentityFields } from "@/features/agents/ui/AgentDescriptionField";
import { evictUsersBatchEntries } from "@/features/profile/hooks";
import { setExternalAgentProfile } from "@/shared/api/agentPairing";
import { getUserProfile } from "@/shared/api/tauriProfiles";
import { useT } from "@/shared/i18n";
import { Button } from "@/shared/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui/dialog";

/**
 * A paired agent's name, picture and description. The relay keeps them, so
 * this works from any device while the agent's computer is off.
 */
export function EditExternalProfileDialog({
  agent,
  onOpenChange,
}: {
  agent: { pubkey: string; name: string };
  onOpenChange: (open: boolean) => void;
}) {
  const t = useT();
  const queryClient = useQueryClient();
  // Not the shared ["user-profile"] entry: a batch read seeds it without the
  // description, and saving replaces all three fields.
  const profile = useQuery({
    gcTime: 0,
    queryFn: () => getUserProfile(agent.pubkey),
    queryKey: ["external-agent-profile", agent.pubkey.toLowerCase()],
    staleTime: 0,
  });
  const [displayName, setDisplayName] = React.useState(agent.name);
  const [avatarUrl, setAvatarUrl] = React.useState("");
  const [description, setDescription] = React.useState("");
  const [uploading, setUploading] = React.useState(false);
  const [saving, setSaving] = React.useState(false);
  const [saveError, setSaveError] = React.useState<string | null>(null);

  // Filled once from a settled read: saving replaces all three, and a later
  // refetch must not overwrite what is being typed.
  const [loaded, setLoaded] = React.useState(false);
  React.useEffect(() => {
    if (loaded || !profile.isSuccess || profile.isFetching) return;
    if (profile.data) {
      setDisplayName(profile.data.displayName?.trim() || agent.name);
      setAvatarUrl(profile.data.avatarUrl ?? "");
      setDescription(clampAgentDescription(profile.data.about ?? ""));
    }
    setLoaded(true);
  }, [agent.name, loaded, profile.data, profile.isFetching, profile.isSuccess]);

  const name = displayName.trim();
  const disabled = !loaded || saving || uploading;

  async function save() {
    setSaving(true);
    setSaveError(null);
    try {
      await setExternalAgentProfile(agent.pubkey, {
        displayName: name,
        avatarUrl: avatarUrl.trim() || null,
        about: description.trim() || null,
      });
      const pubkey = agent.pubkey.toLowerCase();
      evictUsersBatchEntries(queryClient, [pubkey]);
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["user-profile", pubkey] }),
        queryClient.invalidateQueries({
          predicate: (query) =>
            query.queryKey[0] === "users-batch" &&
            query.queryKey.includes(pubkey),
        }),
      ]);
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
        className="max-w-2xl"
        data-testid="external-agent-profile-dialog"
      >
        <DialogHeader>
          <DialogTitle>{t("agents.external.editProfile")}</DialogTitle>
          <DialogDescription>
            {t("agents.external.editProfileBody", { name: agent.name })}
          </DialogDescription>
        </DialogHeader>
        <div className="grid gap-5 sm:grid-cols-[180px_minmax(0,1fr)]">
          <AgentCreationPreview
            avatarUrl={avatarUrl.trim() || null}
            disabled={disabled}
            label={name || agent.name}
            onClearAvatar={() => setAvatarUrl("")}
            onSelectAvatar={setAvatarUrl}
            onUploadPendingChange={setUploading}
          />
          <div className="space-y-5">
            <AgentIdentityFields
              description={description}
              disabled={!loaded || saving}
              displayName={displayName}
              onDescriptionChange={setDescription}
              onDisplayNameChange={setDisplayName}
            />
          </div>
        </div>
        {saveError || profile.error ? (
          <p className="text-sm text-destructive">
            {saveError ?? String(profile.error)}
          </p>
        ) : null}
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
            data-testid="external-agent-profile-save"
            disabled={disabled || !name}
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
