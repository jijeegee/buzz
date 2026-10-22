/**
 * Own messages the relay did not accept. The row stays in the timeline with a
 * failed mark; these entries carry what the row's Retry / Delete buttons do.
 * Keyed by the optimistic row's local key. Community-scoped: reset on switch.
 */
type FailedSend = {
  retry: () => void;
  discard: () => void;
};

const failedSends = new Map<string, FailedSend>();

export function registerFailedSend(localKey: string, entry: FailedSend) {
  failedSends.set(localKey, entry);
}

export function hasFailedSend(localKey: string): boolean {
  return failedSends.has(localKey);
}

/** Let the sender's flow supply a richer retry for a row it handed off. */
export function replaceFailedSendRetry(localKey: string, retry: () => void) {
  const entry = failedSends.get(localKey);
  if (entry) failedSends.set(localKey, { ...entry, retry });
}

/** Re-send a failed row. The row returns to pending until the relay answers. */
export function retryFailedSend(localKey: string) {
  const entry = failedSends.get(localKey);
  if (!entry) return;
  failedSends.delete(localKey);
  entry.retry();
}

/** Remove a failed row without sending it. */
export function discardFailedSend(localKey: string) {
  const entry = failedSends.get(localKey);
  if (!entry) return;
  failedSends.delete(localKey);
  entry.discard();
}

export function resetFailedSends() {
  failedSends.clear();
}
