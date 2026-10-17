import { EyeOff } from "lucide-react";

export const BLOCKED_MESSAGE_LABEL = "Message from a blocked user";

/**
 * Stands in for a message whose author I blocked (shared channels still
 * deliver them). One line; clicking it shows the message.
 */
export function BlockedMessageNotice({ onShow }: { onShow: () => void }) {
  return (
    <button
      aria-label={`${BLOCKED_MESSAGE_LABEL}. Show it`}
      className="mx-1 flex w-fit items-center gap-2 rounded-md px-2 py-1 text-left text-xs text-muted-foreground transition-colors hover:bg-muted/60 hover:text-foreground focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring"
      data-testid="blocked-message-notice"
      onClick={onShow}
      type="button"
    >
      <EyeOff aria-hidden className="h-3.5 w-3.5 shrink-0" />
      <span>{BLOCKED_MESSAGE_LABEL}</span>
      <span className="font-medium underline-offset-2 hover:underline">
        Show
      </span>
    </button>
  );
}
