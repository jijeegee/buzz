import type { RespondToMode } from "@/shared/api/types";
import {
  CreateAgentRespondToField,
  ownerOnlyAccessDisabledReason,
} from "./RespondToField";

export function OwnerOnlyAccessField({
  accessLocked,
  allowlist,
  disabled,
  mode,
  onAllowlistChange,
  onModeChange,
}: {
  accessLocked: boolean;
  allowlist: string[];
  disabled: boolean;
  mode: RespondToMode;
  onAllowlistChange: (allowlist: string[]) => void;
  onModeChange: (mode: RespondToMode) => void;
}) {
  return (
    <CreateAgentRespondToField
      allowlist={accessLocked ? [] : allowlist}
      disabled={disabled || accessLocked}
      disabledReason={
        accessLocked ? ownerOnlyAccessDisabledReason() : undefined
      }
      mode={accessLocked ? "owner-only" : mode}
      onAllowlistChange={onAllowlistChange}
      onModeChange={onModeChange}
      variant="persona"
    />
  );
}
