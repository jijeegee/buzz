import { useExportAgentSnapshotMutation } from "@/features/agents/hooks";
import { AgentSnapshotExportDialog } from "@/features/agents/ui/AgentSnapshotExportDialog";
import type { AgentPersona } from "@/shared/api/types";
import { toast } from "sonner";
import { useT } from "@/shared/i18n";

export function UserProfileSnapshotExportDialog({
  persona,
  linkedAgentPubkey,
  onOpenChange,
}: {
  persona: AgentPersona;
  linkedAgentPubkey: string | null;
  onOpenChange: (open: boolean) => void;
}) {
  const t = useT();
  const exportSnapshotMutation = useExportAgentSnapshotMutation();

  return (
    <AgentSnapshotExportDialog
      agentName={persona.displayName}
      isSavePending={exportSnapshotMutation.isPending}
      linkedAgentPubkey={linkedAgentPubkey}
      open
      onOpenChange={onOpenChange}
      onSaveFile={(memoryLevel, format) => {
        exportSnapshotMutation.mutate(
          {
            id: persona.id,
            memoryLevel,
            format,
            memorySourcePubkey: linkedAgentPubkey,
            avatarUrl: persona.avatarUrl,
          },
          {
            onSuccess: (saved) => {
              if (saved) {
                toast.success(
                  t("profile.snapshotExport.exported", {
                    name: persona.displayName,
                  }),
                );
                onOpenChange(false);
              }
            },
            onError: (error) => {
              toast.error(
                error instanceof Error
                  ? error.message
                  : t("profile.snapshotExport.failed"),
              );
            },
          },
        );
      }}
    />
  );
}
