import { toast } from "sonner";

export function showAgentProfileSyncWarning(
  agentName: string,
  profileSyncError: string | null,
) {
  if (!profileSyncError) return;
  toast.warning(
    `${agentName} was saved on this device, but couldn't sync: ${profileSyncError}. Others may still see the previous name or access policy until Kuzz retries the sync.`,
  );
}
