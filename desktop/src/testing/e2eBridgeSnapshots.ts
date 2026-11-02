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

/**
 * The user stream's store calls (docs/message-sync.md#user-stream): the mock
 * keeps the epoch and stream state for the page's lifetime and no rows.
 */
const streamState = new Map<string, string>();

export function handleStreamStoreMockCommand(
  command: string,
  rawPayload: unknown,
): { value: unknown } | null {
  const payload = (rawPayload ?? {}) as Record<string, unknown>;
  switch (command) {
    case "message_store_stream_epoch":
      streamState.set("stream_epoch", String(payload.epoch));
      return { value: true };
    case "message_store_stream_seqs":
      return {
        value: { epoch: streamState.get("stream_epoch") ?? null, rows: {} },
      };
    case "message_store_clear_stream_rows":
    case "message_store_set_stream_state":
      if (command === "message_store_set_stream_state") {
        streamState.set(String(payload.key), String(payload.value));
      }
      return { value: true };
    case "message_store_extend_stream_coverage":
    case "message_store_walk_baseline":
      return { value: null };
    case "message_store_stream_state":
      return { value: streamState.get(String(payload.key)) ?? null };
    default:
      return null;
  }
}
