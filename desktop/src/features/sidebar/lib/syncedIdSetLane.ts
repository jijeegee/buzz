import * as React from "react";

import { normalizeRelayUrl } from "@/shared/lib/normalizeRelayUrl";
import { useLaneSync, type Lane } from "./sidebarLaneReconciler";
import { isReg, own, setRegs, type Tree } from "./sidebarLwwMap";

/**
 * A synced set of ids (encrypted-to-self kind 30078, like channel stars and
 * sections): one boolean register per id, so devices merge per id. Removal
 * writes `false`, which wins over an older `true`.
 */
export function createIdSetLane({
  dTag,
  maxEntries,
  storagePrefix,
}: {
  dTag: string;
  maxEntries: number;
  storagePrefix: string;
}): Lane {
  return {
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

/** Writes one id's membership; a no-op when it already holds that value. */
export function setIdInTree(tree: Tree, id: string, member: boolean): Tree {
  const node = tree.i;
  if (!member && (!node || isReg(node) || own(node, id) === undefined)) {
    return tree;
  }
  return setRegs(tree, [[["i", id], member]]);
}

export function useSyncedIdSet(
  lane: Lane,
  pubkey: string | undefined,
  relayUrl: string | undefined,
): {
  ids: ReadonlySet<string>;
  add: (id: string) => void;
  remove: (id: string) => void;
} {
  const { tree, edit } = useLaneSync(lane, pubkey, relayUrl);
  const ids = React.useMemo(() => new Set(idSetFromTree(tree)), [tree]);
  const add = React.useCallback(
    (id: string) => edit((prev) => setIdInTree(prev, id, true)),
    [edit],
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
  maxEntries: 500,
  storagePrefix: "kuzz-people-favorites.v1",
});

/** Threads pinned from a Chats row. */
export const THREAD_PINS_LANE = createIdSetLane({
  dTag: "thread-pins",
  maxEntries: 500,
  storagePrefix: "kuzz-thread-pins.v1",
});
