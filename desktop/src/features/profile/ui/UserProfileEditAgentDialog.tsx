import { AgentDialog } from "@/features/agents/ui/AgentDialog";
import type { EditAgentFocusTarget } from "@/features/agents/openEditAgentEvent";
import type { ManagedAgent } from "@/shared/api/types";

export function UserProfileEditAgentDialog({
  agent,
  canEdit,
  initialFocus,
  onOpenChange,
  open,
}: {
  agent: ManagedAgent | undefined;
  canEdit: boolean;
  initialFocus: EditAgentFocusTarget | undefined;
  onOpenChange: (open: boolean) => void;
  open: boolean;
}) {
  if (!canEdit || !agent) {
    return null;
  }

  return (
    <AgentDialog
      agent={agent}
      initialFocus={initialFocus}
      mode="instance-edit"
      onOpenChange={onOpenChange}
      open={open}
    />
  );
}
