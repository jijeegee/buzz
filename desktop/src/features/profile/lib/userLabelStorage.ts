/**
 * Other people's labels and avatar URLs kept from earlier runs, so a launch
 * draws known names and pictures at once instead of waiting for the relay
 * (docs/message-sync.md#launch-snapshots). Kept per account and server in
 * the device message store (`profiles` snapshot, the 500 most recent), read
 * once into memory when the community's cache is made; relay answers
 * overwrite entries and are saved back shortly after. Lookups are
 * synchronous and only ever see the asking account's entries.
 *
 * Earlier builds kept them in localStorage per server (`kuzz-user-labels.v1`,
 * no account split). When the account opening that server is the only one
 * signed in there, it takes them over; otherwise nobody does. Either way the
 * old key is deleted.
 *
 * A save waits until its scope's kept labels have been read, and is built
 * when it is written, so it always holds everything known by then.
 */
import { normalizeRelayUrl } from "@/features/profile/lib/selfProfileStorage";
import type {
  UserProfileSummary,
  UsersBatchResponse,
} from "@/shared/api/types";
import {
  createSnapshotWriter,
  isSoleAccountOnServer,
  readDeviceSnapshot,
  readSignedInDeviceSnapshot,
  type SnapshotScope,
  writeDeviceSnapshot,
} from "@/shared/api/deviceSnapshot";

const LEGACY_KEY_PREFIX = "kuzz-user-labels.v1";
export const MAX_CACHED_LABELS = 500;
const SAVE_DELAY_MS = 1_500;

type CachedUserLabel = {
  displayName: string | null;
  name: string | null;
  nip05Handle: string | null;
  /** Kept so a restart draws a known avatar at once. */
  avatarUrl: string | null;
  updatedAt: number;
};

type UserLabelCache = {
  version: 1;
  profiles: Record<string, CachedUserLabel>;
};

/** The account and server whose kept labels are read or written. */
export type UserLabelScope = SnapshotScope;

/** The localStorage key earlier builds used for `relayUrl`. */
export function userLabelCacheKey(relayUrl: string): string {
  return `${LEGACY_KEY_PREFIX}:${normalizeRelayUrl(relayUrl)}`;
}

function scopeKey(scope: UserLabelScope): string {
  return `${normalizeRelayUrl(scope.relayUrl)}|${scope.pubkey.toLowerCase()}`;
}

function nullableString(value: unknown): string | null | undefined {
  if (value === null || value === undefined) return null;
  return typeof value === "string" ? value : undefined;
}

function parseCachedUserLabel(value: unknown): CachedUserLabel | null {
  if (typeof value !== "object" || value === null) return null;
  const raw = value as Record<string, unknown>;
  const displayName = nullableString(raw.displayName);
  const name = nullableString(raw.name);
  const nip05Handle = nullableString(raw.nip05Handle);
  const avatarUrl = nullableString(raw.avatarUrl) ?? null;
  if (
    displayName === undefined ||
    name === undefined ||
    nip05Handle === undefined
  ) {
    return null;
  }
  if (
    ![displayName, name, nip05Handle, avatarUrl].some((label) => label?.trim())
  ) {
    return null;
  }
  return {
    displayName,
    name,
    nip05Handle,
    avatarUrl,
    updatedAt:
      typeof raw.updatedAt === "number" && Number.isFinite(raw.updatedAt)
        ? raw.updatedAt
        : 0,
  };
}

/** The labels of a stored blob; null when it is not one. */
export function parseUserLabelCache(
  raw: string | null,
): Record<string, CachedUserLabel> | null {
  try {
    if (!raw) return null;
    const parsed = JSON.parse(raw) as unknown;
    if (typeof parsed !== "object" || parsed === null) return null;
    const payload = parsed as Record<string, unknown>;
    if (
      payload.version !== 1 ||
      typeof payload.profiles !== "object" ||
      payload.profiles === null
    ) {
      return null;
    }
    const profiles: Record<string, CachedUserLabel> = {};
    for (const [pubkey, value] of Object.entries(
      payload.profiles as Record<string, unknown>,
    )) {
      const label = parseCachedUserLabel(value);
      if (label) profiles[pubkey.toLowerCase()] = label;
    }
    return profiles;
  } catch {
    return null;
  }
}

/** Kept labels per account and server, as read and updated this run. */
const labels = new Map<string, Map<string, CachedUserLabel>>();
/** Reads per scope this run: the active one's, and watched ones' (read only). */
const hydrations = new Map<string, Promise<void>>();
const watched = new Map<string, Promise<void>>();
const listeners = new Set<() => void>();
let version = 0;
const writer = createSnapshotWriter("profiles", SAVE_DELAY_MS, {
  ready: (scope) => hydrateUserLabels(scope),
});

function labelsOf(scope: UserLabelScope) {
  const key = scopeKey(scope);
  let map = labels.get(key);
  if (!map) {
    map = new Map();
    labels.set(key, map);
  }
  return map;
}

/** Bumped when kept labels arrive from the device; for re-rendering. */
export function getUserLabelsVersion(): number {
  return version;
}

export function subscribeUserLabels(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function bounded(map: Map<string, CachedUserLabel>) {
  if (map.size <= MAX_CACHED_LABELS) return;
  const newest = [...map.entries()]
    .sort(([, left], [, right]) => right.updatedAt - left.updatedAt)
    .slice(0, MAX_CACHED_LABELS);
  map.clear();
  for (const [pubkey, label] of newest) map.set(pubkey, label);
}

function serialize(map: Map<string, CachedUserLabel>): string {
  return JSON.stringify({
    version: 1,
    profiles: Object.fromEntries(map),
  } satisfies UserLabelCache);
}

/** Merges `stored` under what this run already knows (newer wins). */
function mergeStored(
  map: Map<string, CachedUserLabel>,
  stored: Record<string, CachedUserLabel>,
) {
  for (const [pubkey, label] of Object.entries(stored)) {
    const known = map.get(pubkey);
    if (!known || known.updatedAt < label.updatedAt) map.set(pubkey, label);
  }
  bounded(map);
}

function readLegacy(relayUrl: string) {
  try {
    return parseUserLabelCache(
      window.localStorage.getItem(userLabelCacheKey(relayUrl)),
    );
  } catch {
    return null;
  }
}

function removeLegacy(relayUrl: string) {
  try {
    window.localStorage.removeItem(userLabelCacheKey(relayUrl));
  } catch {
    // Storage access failures are non-fatal.
  }
}

function changed() {
  version += 1;
  for (const listener of listeners) listener();
}

/** Earlier builds' labels for the server: taken over by a sole account. */
async function takeOverLegacy(scope: UserLabelScope) {
  const legacy = readLegacy(scope.relayUrl);
  if (!legacy) return;
  if (!(await isSoleAccountOnServer(scope))) {
    removeLegacy(scope.relayUrl);
    return;
  }
  const map = labelsOf(scope);
  mergeStored(map, legacy);
  // The merged set: anything relay answers added meanwhile is kept too.
  if (await writeDeviceSnapshot("profiles", scope, serialize(map))) {
    removeLegacy(scope.relayUrl);
  }
}

/**
 * Reads the active `scope`'s kept labels into memory, once per scope and
 * activation; saves for it wait for this. With none kept yet, takes over
 * the server's labels from earlier builds (see the file comment).
 */
export function hydrateUserLabels(scope: UserLabelScope): Promise<void> {
  const key = scopeKey(scope);
  const running = hydrations.get(key);
  if (running) return running;
  const hydration = (async () => {
    const stored = parseUserLabelCache(
      await readDeviceSnapshot("profiles", scope),
    );
    if (stored) mergeStored(labelsOf(scope), stored);
    else await takeOverLegacy(scope);
    changed();
  })();
  hydrations.set(key, hydration);
  return hydration;
}

/**
 * Reads the kept labels of an account watched in the background (signed in
 * here, not the active one), once per run. Read only: nothing is saved or
 * taken over for it.
 */
export function hydrateWatchedUserLabels(scope: UserLabelScope): Promise<void> {
  const key = scopeKey(scope);
  const running = watched.get(key);
  if (running) return running;
  const hydration = (async () => {
    const stored = parseUserLabelCache(
      await readSignedInDeviceSnapshot("profiles", scope),
    );
    if (!stored) return;
    mergeStored(labelsOf(scope), stored);
    changed();
  })();
  watched.set(key, hydration);
  return hydration;
}

export function readCachedUserLabels(
  scope: UserLabelScope | null,
  pubkeys: string[],
): UsersBatchResponse | undefined {
  if (!scope) return undefined;
  const map = labels.get(scopeKey(scope));
  if (!map || map.size === 0) return undefined;
  const profiles: UsersBatchResponse["profiles"] = {};
  for (const pubkey of pubkeys) {
    const normalizedPubkey = pubkey.toLowerCase();
    const cached = map.get(normalizedPubkey);
    if (!cached) continue;
    profiles[normalizedPubkey] = {
      displayName: cached.displayName,
      name: cached.name,
      avatarUrl: cached.avatarUrl,
      nip05Handle: cached.nip05Handle,
      ownerPubkey: null,
    };
  }
  return Object.keys(profiles).length > 0
    ? { profiles, missing: [] }
    : undefined;
}

export function resolveUserLabelPlaceholderData(
  previousData: UsersBatchResponse | undefined,
  scope: UserLabelScope | null,
  pubkeys: string[],
): UsersBatchResponse | undefined {
  return previousData ?? readCachedUserLabels(scope, pubkeys);
}

/** Relay answers overwrite kept labels; saved to the device shortly after. */
export function writeCachedUserLabels(
  scope: UserLabelScope | null,
  profiles: Record<string, UserProfileSummary>,
  missing: string[] = [],
): void {
  if (!scope?.pubkey || !scope.relayUrl) return;
  const now = Date.now();
  const map = labelsOf(scope);
  for (const [pubkey, profile] of Object.entries(profiles)) {
    const label = parseCachedUserLabel({
      displayName: profile.displayName,
      name: profile.name,
      nip05Handle: profile.nip05Handle,
      avatarUrl: profile.avatarUrl,
      updatedAt: now,
    });
    const normalizedPubkey = pubkey.toLowerCase();
    if (label) map.set(normalizedPubkey, label);
    else map.delete(normalizedPubkey);
  }
  for (const pubkey of missing) map.delete(pubkey.toLowerCase());
  bounded(map);
  writer.save(scope, () => serialize(labelsOf(scope)));
}

/** A removed community: forget its labels here and from earlier builds. */
export function removeUserLabelCacheForRelay(relayUrl: string): void {
  const prefix = `${normalizeRelayUrl(relayUrl)}|`;
  for (const key of [...labels.keys()]) {
    if (key.startsWith(prefix)) {
      labels.delete(key);
      hydrations.delete(key);
      watched.delete(key);
    }
  }
  removeLegacy(relayUrl);
}

/**
 * The active community changed: saves still waiting are written now, while
 * their scope may still be active (the store refuses them otherwise), and
 * the next activation of a scope reads the device again. Labels stay per
 * scope in memory, so no account ever sees another's.
 */
export function resetUserLabelStorageCache(): void {
  // Each waiting save takes its scope's read before it is cleared below.
  void writer.flushAll();
  hydrations.clear();
}

/** Test seam: forget everything kept in memory. */
export function resetUserLabelsForTests(): void {
  writer.reset();
  labels.clear();
  hydrations.clear();
  watched.clear();
  version = 0;
}
