import { useQueries, useQueryClient } from "@tanstack/react-query";

import { storeScope } from "@/features/messages/lib/paintCoverage";
import { threadRootVerdict } from "@/features/messages/lib/threadRootResolve";
import { getEventById, invokeTauri } from "@/shared/api/tauri";
import type { RelayEvent } from "@/shared/api/types";
import { isDefinitiveEventNotFound } from "@/shared/lib/eventLookupError";

/**
 * Root-existence status for a thread-draft's parent event.
 *
 * - `checking`   — query in flight. Treated as available/counted (optimistic)
 *                  so a slow relay does not blink drafts out of the badge.
 * - `available`  — `getEventById` resolved; root exists.
 * - `deleted`    — ONLY on an explicit deletion of the root (a removal, or
 *                  the author's deletion when the author is known) returned
 *                  after `get_event` found no root. Excluded from the badge
 *                  count; open+send disabled; "thread deleted" label shown.
 *                  A root merely not found (e.g. from before the member
 *                  joined a since-join room) stays `available`.
 * - `error`      — any other rejection (transport/auth/serialize). Treated
 *                  as available/counted; NOT labeled deleted. Re-checked on
 *                  next panel open.
 */
export type RootStatus = "checking" | "available" | "deleted" | "error";

type DraftRootDeps = {
  getEvent: (rootId: string) => Promise<RelayEvent>;
  /** The relay's answer to the root and deletion filters; null = failed. */
  askRoot: (channelId: string, rootId: string) => Promise<RelayEvent[] | null>;
};

/**
 * `available` or `deleted` for a draft's thread root; a failed lookup throws
 * (status `error`). Not found is deleted only on an explicit deletion.
 */
export async function resolveDraftRootStatus(
  rootId: string,
  channelId: string | null,
  deps: DraftRootDeps,
): Promise<"available" | "deleted"> {
  try {
    await deps.getEvent(rootId);
    return "available";
  } catch (error) {
    if (!isDefinitiveEventNotFound(error)) throw error;
  }
  if (!channelId) return "available";
  const answer = await deps.askRoot(channelId, rootId);
  return threadRootVerdict(answer, { channelId, rootId }).verdict === "deleted"
    ? "deleted"
    : "available";
}

/**
 * Resolves root-existence status for a set of thread-root event IDs.
 *
 * **Query semantics (locked — Thufir will verify):**
 * - `staleTime: 0` — every query is always considered stale.
 * - `refetchOnMount: "always"` — re-fetch every time this hook mounts (i.e.
 *   every time the Drafts panel opens), so the flag is genuinely per-panel-open
 *   and a restored root flips `deleted` → `available` on next open.
 * - Query key = `["draftRootStatus", rootId]` — root id only, no timestamp.
 *   React Query dedupes concurrent lookups within one open; never serves an
 *   indefinitely-stale `deleted`.
 * - `deleted` is NEVER written into any persisted cache — it is a pure
 *   read-through of the current lookup.
 * - Enabled only when `isOpen` is true so we don't burn relay RTTs when the
 *   Drafts panel is closed.
 *
 * @param rootIds  Deduplicated thread-root event IDs to check.
 * @param isOpen   Whether the Drafts panel is currently visible.
 * @returns        A `Map<rootId, RootStatus>` — one entry per input root id.
 */
export function useDraftRootStatus(
  rootIds: string[],
  isOpen: boolean,
  channelIdByRootId: ReadonlyMap<string, string> = new Map(),
): Map<string, RootStatus> {
  const queryClient = useQueryClient();
  const deps: DraftRootDeps = {
    getEvent: getEventById,
    askRoot: async (channelId, rootId) => {
      const scope = storeScope(queryClient);
      if (!scope) return null;
      try {
        return await invokeTauri<RelayEvent[]>("get_thread_root", {
          channelId,
          rootId,
          account: scope.pubkey,
          relayUrl: scope.relayUrl,
        });
      } catch {
        return null;
      }
    },
  };
  const results = useQueries({
    queries: rootIds.map((rootId) => ({
      queryKey: ["draftRootStatus", rootId] as const,
      queryFn: () =>
        resolveDraftRootStatus(
          rootId,
          channelIdByRootId.get(rootId) ?? null,
          deps,
        ),
      staleTime: 0,
      refetchOnMount: "always" as const,
      retry: false,
      enabled: isOpen && rootId.length > 0,
    })),
  });

  const statusMap = new Map<string, RootStatus>();
  rootIds.forEach((rootId, index) => {
    const result = results[index];
    if (!result) {
      statusMap.set(rootId, "checking");
      return;
    }
    if (result.isPending || result.isFetching) {
      statusMap.set(rootId, "checking");
    } else if (result.isError) {
      statusMap.set(rootId, "error");
    } else {
      statusMap.set(rootId, result.data ?? "available");
    }
  });

  return statusMap;
}
