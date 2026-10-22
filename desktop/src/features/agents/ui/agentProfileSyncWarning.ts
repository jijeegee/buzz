import { toast } from "sonner";
import { t } from "@/shared/i18n";

export function showAgentProfileSyncWarning(
  agentName: string,
  profileSyncError: string | null,
) {
  if (!profileSyncError) return;
  toast.warning(
    t("agents.profileSync.warning", {
      name: agentName,
      error: profileSyncError,
    }),
  );
}
