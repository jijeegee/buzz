/**
 * Which preload rooms need the relay (docs/message-sync.md#store-first-preload).
 * A room whose live REQ has reached EOSE (it has a token) is asked of the
 * store first; a certain one is left for its open to draw from the store.
 * Every other room is fetched. Mirrors mobile's `store_preload_plan.dart`;
 * shared cases: test-fixtures/message-store-preload-plan.json.
 */
export type StorePreloadPlan = {
  /** Rooms to ask the store about (those with a token), in order. */
  check: string[];
  /** Rooms not requested: an open draws them from the store. */
  certain: string[];
  /** Rooms requested from the relay, in order. */
  fetch: string[];
  /** The fetched rooms that had no token, so were never checked. */
  noToken: string[];
};

export function planStorePreload(
  rooms: readonly string[],
  tokens: ReadonlySet<string>,
  certain: ReadonlySet<string>,
): StorePreloadPlan {
  const plan: StorePreloadPlan = {
    check: [],
    certain: [],
    fetch: [],
    noToken: [],
  };
  for (const room of new Set(rooms)) {
    if (!tokens.has(room)) {
      plan.fetch.push(room);
      plan.noToken.push(room);
      continue;
    }
    plan.check.push(room);
    if (certain.has(room)) plan.certain.push(room);
    else plan.fetch.push(room);
  }
  return plan;
}

/** The one log line of a preload pass. */
export function storePreloadSummary(plan: StorePreloadPlan) {
  return `[preload] certain ${plan.certain.length} / fetched ${plan.fetch.length} / skipped (no token) ${plan.noToken.length}`;
}
