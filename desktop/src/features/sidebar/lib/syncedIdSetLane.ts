import * as React from "react";
import { toast } from "sonner";

import { type MessageKey, t } from "@/shared/i18n";
import { normalizeRelayUrl } from "@/shared/lib/normalizeRelayUrl";
import { useLaneSync, type Lane } from "./sidebarLaneReconciler";
import { isReg, own, setRegs, type Reg, type Tree } from "./sidebarLwwMap";

/**
 * Removals kept as `false` registers so a removal reaches devices that still
 * hold the id. Only the most recent ones are kept: with up to 200 ids and 50
 * removals the encrypted record stays well under the relay's 65,535-byte
 * limit. A device offline across more than 50 later removals could bring an
 * old one back.
 */
export const MAX_ID_SET_TOMBSTONES = 50;

export type IdSetLane = Lane & {
  maxEntries: number;
  /** Language-file key of the toast shown when adding past `maxEntries`. */
  limitMessageKey: MessageKey;
};

/**
 * A synced set of ids (encrypted-to-self kind 30078, like channel stars and
 * sections): one boolean register per id, so devices merge per id. Removal
 * writes `false`, which wins over an older `true`.
 */
export function createIdSetLane({
  dTag,
  limitMessageKey,
  maxEntries,
  storagePrefix,
}: {
  dTag: string;
  limitMessageKey: MessageKey;
  maxEntries: number;
  storagePrefix: string;
}): IdSetLane {
  return {
    limitMessageKey,
    maxEntries,
    dTag,
    storageKey: (pubkey, relayUrl) =>
      `${storagePrefix}:${pubkey}:${encodeURIComponent(normalizeRelayUrl(relayUrl))}`,
    shape: { i: { "*": (value) => typeof value === "boolean" } },
    // New lanes have no meta-less writers to import.
    fromLegacy: () => null,
    project: (tree) => ({ ids: idSetFromTree(tree) }),
    withinLimits: (projection) =>
      (projection.ids as unknown[]).length <= maxEntries,
  };
}

/** Ids whose register is `true`, sorted for a stable projection. */
export function idSetFromTree(tree: Tree): string[] {
  const node = tree.i;
  if (!node || isReg(node)) return [];
  return Object.entries(node)
    .filter(([, reg]) => isReg(reg) && reg[2] === true)
    .map(([id]) => id)
    .sort();
}

/**
 * Writes one id's membership; a no-op when it already holds that value.
 * Beyond `maxTombstones` removals, the oldest `false` registers are dropped.
 */
export function setIdInTree(
  tree: Tree,
  id: string,
  member: boolean,
  maxTombstones = MAX_ID_SET_TOMBSTONES,
): Tree {
  const node = tree.i;
  if (!member && (!node || isReg(node) || own(node, id) === undefined)) {
    return tree;
  }
  return pruneTombstones(setRegs(tree, [[["i", id], member]]), maxTombstones);
}

function pruneTombstones(tree: Tree, maxTombstones: number): Tree {
  const node = tree.i;
  if (!node || isReg(node)) return tree;
  const tombstones = Object.entries(node)
    .filter((entry): entry is [string, Reg] => {
      const reg = entry[1];
      return isReg(reg) && reg[2] === false;
    })
    .sort(([, a], [, b]) => b[0] - a[0]);
  if (tombstones.length <= maxTombstones) return tree;
  const next: Tree = { ...node };
  for (const [key] of tombstones.slice(maxTombstones)) delete next[key];
  return { ...tree, i: next };
}

export function useSyncedIdSet(
  lane: IdSetLane,
  pubkey: string | undefined,
  relayUrl: string | undefined,
): {
  ids: ReadonlySet<string>;
  /** False (with a toast) when the set is full. */
  add: (id: string) => boolean;
  remove: (id: string) => void;
} {
  const { tree, edit } = useLaneSync(lane, pubkey, relayUrl);
  const ids = React.useMemo(() => new Set(idSetFromTree(tree)), [tree]);
  const add = React.useCallback(
    (id: string) => {
      if (!ids.has(id) && ids.size >= lane.maxEntries) {
        toast.error(t(lane.limitMessageKey));
        return false;
      }
      edit((prev) => setIdInTree(prev, id, true));
      return true;
    },
    [edit, ids, lane],
  );
  const remove = React.useCallback(
    (id: string) => edit((prev) => setIdInTree(prev, id, false)),
    [edit],
  );
  return { ids, add, remove };
}

/** People starred on the People screen or a profile. */
export const PEOPLE_FAVORITES_LANE = createIdSetLane({
  dTag: "people-favorites",
  limitMessageKey: "sidebar.limits.favoritePeople",
  maxEntries: 200,
  storagePrefix: "kuzz-people-favorites.v1",
});

/** Threads pinned from a Chats row; the relay lists up to 200 pinned threads. */
export const THREAD_PINS_LANE = createIdSetLane({
  dTag: "thread-pins",
  limitMessageKey: "sidebar.limits.pinThreads",
  maxEntries: 200,
  storagePrefix: "kuzz-thread-pins.v1",
});
