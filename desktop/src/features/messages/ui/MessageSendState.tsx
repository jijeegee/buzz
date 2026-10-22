import { AlertCircle, Clock } from "lucide-react";

import {
  discardFailedSend,
  retryFailedSend,
} from "@/features/messages/lib/failedSends";

/** Send state of an own message, shown where its time goes once sent. */
export function MessageSendState({ failed }: { failed: boolean }) {
  return failed ? (
    <AlertCircle
      aria-label="Not sent"
      className="h-3.5 w-3.5 text-destructive"
      data-testid="message-send-failed"
      role="img"
    />
  ) : (
    <Clock
      aria-label="Sending"
      className="h-3 w-3 text-muted-foreground/70"
      data-testid="message-send-pending"
      role="img"
    />
  );
}

/** Retry / Delete under an own message the relay did not accept. */
export function FailedSendActions({ localKey }: { localKey: string }) {
  return (
    <span
      className="flex items-baseline gap-1.5 text-destructive"
      data-testid="message-send-status"
    >
      Not sent
      <button
        className="font-medium underline-offset-2 hover:underline"
        onClick={() => retryFailedSend(localKey)}
        type="button"
      >
        Retry
      </button>
      <button
        className="font-medium underline-offset-2 hover:underline"
        onClick={() => discardFailedSend(localKey)}
        type="button"
      >
        Delete
      </button>
    </span>
  );
}
