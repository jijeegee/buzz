import {
  THREAD_CLOSED_CONTENT,
  THREAD_REOPENED_CONTENT,
} from "@/features/messages/lib/taskThread";
import type { sendChannelMessage } from "@/shared/api/tauriMessages";

export type ThreadMarker = "closed" | "reopened";

/** The account a marker is bound to, captured when the user pressed Close. */
export type ThreadMarkerScope = {
  relayUrl: string | null | undefined;
  signerPubkey: string | null | undefined;
};

/**
 * Sends a Close/Reopen marker reply bound to `scope`, like other sends that
 * cross an await: the backend refuses it if the active community or signing
 * identity changed since the scope was captured, so an account switch
 * mid-action (or before Undo) can never sign the marker as someone else.
 * Without a full scope nothing is sent.
 */
export function sendThreadMarker(
  send: typeof sendChannelMessage,
  {
    channelId,
    marker,
    rootId,
    scope,
  }: {
    channelId: string;
    marker: ThreadMarker;
    rootId: string;
    scope: ThreadMarkerScope;
  },
) {
  const relayUrl = scope.relayUrl?.trim();
  const signerPubkey = scope.signerPubkey?.trim().toLowerCase();
  if (!relayUrl || !signerPubkey) {
    return Promise.reject(new Error("No active account to update the thread."));
  }
  return send(
    channelId,
    marker === "closed" ? THREAD_CLOSED_CONTENT : THREAD_REOPENED_CONTENT,
    rootId,
    undefined,
    undefined,
    undefined,
    undefined,
    undefined,
    undefined,
    undefined,
    relayUrl,
    signerPubkey,
    rootId,
    undefined,
    marker,
  );
}
