/**
 * Launch snapshots kept in the device message store
 * (docs/message-sync.md#launch-snapshots): small blobs (the Chats rows,
 * other people's profiles) drawn before the relay answers. Each lives in the
 * store file of one account on one server, so another account never reads
 * it and signing out deletes it. A read or write names the account and
 * server it is for; the app answers nothing when they are no longer the
 * active ones.
 */
import { invokeTauri } from "@/shared/api/tauri";

export type SnapshotScope = { pubkey: string; relayUrl: string };

export type SnapshotKind = "chat_rows" | "profiles";

type DeviceSnapshotDeps = {
  read: (kind: SnapshotKind, scope: SnapshotScope) => Promise<string | null>;
  /** A watched account's snapshot (signed in here, maybe not active). */
  readSignedIn?: (
    kind: SnapshotKind,
    scope: SnapshotScope,
  ) => Promise<string | null>;
  write: (
    kind: SnapshotKind,
    scope: SnapshotScope,
    blob: string,
  ) => Promise<boolean>;
  /** Whether the active account is the only one signed in on its server. */
  soleAccount?: (scope: SnapshotScope) => Promise<boolean>;
};

const tauriDeps: DeviceSnapshotDeps = {
  read: (kind, scope) =>
    invokeTauri<string | null>("message_store_snapshot_read", {
      kind,
      account: scope.pubkey,
      relayUrl: scope.relayUrl,
    }),
  readSignedIn: (kind, scope) =>
    invokeTauri<string | null>("message_store_snapshot_read_signed_in", {
      kind,
      account: scope.pubkey,
      relayUrl: scope.relayUrl,
    }),
  write: (kind, scope, blob) =>
    invokeTauri<boolean>("message_store_snapshot_write", {
      kind,
      blob,
      account: scope.pubkey,
      relayUrl: scope.relayUrl,
    }),
  soleAccount: (scope) =>
    invokeTauri<boolean>("message_store_sole_account_on_server", {
      account: scope.pubkey,
      relayUrl: scope.relayUrl,
    }),
};

let deps = tauriDeps;

/** Test seam: replace the store calls; no argument restores them. */
export function setDeviceSnapshotDeps(next?: DeviceSnapshotDeps) {
  deps = next ?? tauriDeps;
}

/** Snapshot `kind` of `scope`, or null (none kept, failed, or not active). */
export async function readDeviceSnapshot(
  kind: SnapshotKind,
  scope: SnapshotScope,
): Promise<string | null> {
  try {
    return (await deps.read(kind, scope)) ?? null;
  } catch {
    return null;
  }
}

/**
 * Snapshot `kind` of an account watched in the background: signed in on
 * this computer and not signed out of that server, active or not. Read only.
 */
export async function readSignedInDeviceSnapshot(
  kind: SnapshotKind,
  scope: SnapshotScope,
): Promise<string | null> {
  try {
    return (await deps.readSignedIn?.(kind, scope)) ?? null;
  } catch {
    return null;
  }
}

/** Keeps `blob` as snapshot `kind` of `scope`; false when not written. */
export async function writeDeviceSnapshot(
  kind: SnapshotKind,
  scope: SnapshotScope,
  blob: string,
): Promise<boolean> {
  try {
    return await deps.write(kind, scope, blob);
  } catch {
    return false;
  }
}

/** Whether `scope`'s account is the only one signed in on its server. */
export async function isSoleAccountOnServer(
  scope: SnapshotScope,
): Promise<boolean> {
  try {
    return (await deps.soleAccount?.(scope)) ?? false;
  } catch {
    return false;
  }
}

export function snapshotScopeKey(scope: SnapshotScope): string {
  return `${scope.relayUrl}|${scope.pubkey.toLowerCase()}`;
}

/** A blob, or how to make it when the save is written. */
type SnapshotValue = string | (() => string);

/**
 * Writes the newest value per scope at most once per `delayMs`, skipping a
 * value equal to the last one written. A function value is called when the
 * save is written, so it holds what is known then. With `ready`, a save for
 * a scope waits until that scope's stored snapshot has been read, so it
 * never replaces it with less. `cancel` drops what is waiting.
 */
export function createSnapshotWriter(
  kind: SnapshotKind,
  delayMs: number,
  options: { ready?: (scope: SnapshotScope) => Promise<void> } = {},
) {
  const waiting = new Map<
    string,
    {
      scope: SnapshotScope;
      value: SnapshotValue;
      timer: ReturnType<typeof setTimeout>;
    }
  >();
  const written = new Map<string, string>();
  const flush = async (key: string) => {
    const entry = waiting.get(key);
    if (!entry) return;
    waiting.delete(key);
    clearTimeout(entry.timer);
    if (options.ready) await options.ready(entry.scope);
    const blob =
      typeof entry.value === "function" ? entry.value() : entry.value;
    if (written.get(key) === blob) return;
    written.set(key, blob);
    const ok = await writeDeviceSnapshot(kind, entry.scope, blob);
    if (!ok && written.get(key) === blob) written.delete(key);
  };
  return {
    save(scope: SnapshotScope, value: SnapshotValue) {
      const key = snapshotScopeKey(scope);
      const current = waiting.get(key);
      if (current) {
        current.value = value;
        current.scope = scope;
        return;
      }
      if (typeof value === "string" && written.get(key) === value) return;
      waiting.set(key, {
        scope,
        value,
        timer: setTimeout(() => void flush(key), delayMs),
      });
    },
    /** Writes what is waiting for `scope` now. */
    flush(scope: SnapshotScope): Promise<void> {
      return flush(snapshotScopeKey(scope));
    },
    /** Writes everything waiting now (the store refuses inactive scopes). */
    flushAll(): Promise<void> {
      return Promise.all([...waiting.keys()].map(flush)).then(() => undefined);
    },
    cancel(scope?: SnapshotScope) {
      for (const [key, entry] of waiting) {
        if (scope && key !== snapshotScopeKey(scope)) continue;
        clearTimeout(entry.timer);
        waiting.delete(key);
      }
    },
    /** Forget what was written (tests). */
    reset() {
      this.cancel();
      written.clear();
    },
  };
}
