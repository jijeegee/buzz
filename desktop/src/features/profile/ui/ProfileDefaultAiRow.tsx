import { Star } from "lucide-react";
import * as React from "react";

import { cn } from "@/shared/lib/cn";
import { Switch } from "@/shared/ui/switch";

/**
 * The "Default AI" switch row in the agent profile's Runtime tab (Activity
 * group, directly under "Start on launch"). This is the one user-reachable
 * default-AI control: the header settings menu is never rendered for agents,
 * so a switch there could not be found. Same row pattern as Start on launch —
 * the whole row is the switch; the visual `Switch` is decorative.
 */
export function ProfileDefaultAiRow({
  checked,
  onToggle,
  pending = false,
}: {
  checked: boolean;
  onToggle: () => void;
  pending?: boolean;
}) {
  const handleToggle = React.useCallback(() => {
    if (pending) return;
    onToggle();
  }, [onToggle, pending]);

  return (
    <div
      aria-checked={checked}
      aria-disabled={pending}
      aria-label="Default AI"
      className={cn(
        "flex min-h-16 items-center gap-3 px-4 py-3 transition-colors focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring",
        !pending && "cursor-pointer hover:bg-muted/40",
      )}
      data-testid="user-profile-default-ai"
      onClick={handleToggle}
      onKeyDown={(event) => {
        if (event.key !== "Enter" && event.key !== " ") return;
        event.preventDefault();
        handleToggle();
      }}
      role="switch"
      tabIndex={0}
    >
      <Star
        className="h-4 w-4 shrink-0 text-muted-foreground"
        data-slot="profile-field-icon"
      />
      <span className="min-w-0 flex-1">
        <span className="block text-sm font-medium text-foreground">
          Default AI
        </span>
        <span className="block text-xs text-muted-foreground">
          Offered as a bot on channels you create
        </span>
      </span>
      <Switch
        aria-hidden="true"
        checked={checked}
        data-testid="user-profile-default-ai-toggle"
        disabled={pending}
        tabIndex={-1}
      />
    </div>
  );
}
