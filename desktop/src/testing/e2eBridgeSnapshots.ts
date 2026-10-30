/**
 * Launch snapshots for the mock bridge: one signed-in account, kept for the
 * page's lifetime (docs/message-sync.md#launch-snapshots).
 */
const snapshots = new Map<string, string>();

export function handleSnapshotMockCommand(
  command: string,
  rawPayload: unknown,
): { value: unknown } | null {
  const payload = (rawPayload ?? {}) as Record<string, unknown>;
  const key = `${String(payload.relayUrl)}|${String(payload.account).toLowerCase()}|${String(payload.kind)}`;
  switch (command) {
    case "message_store_snapshot_read":
    case "message_store_snapshot_read_signed_in":
      return { value: snapshots.get(key) ?? null };
    case "message_store_snapshot_write":
      snapshots.set(key, String(payload.blob));
      return { value: true };
    case "message_store_sole_account_on_server":
      return { value: true };
    default:
      return null;
  }
}
