/**
 * Local-only tag marking a timeline event built from a search hit before the
 * relay copy arrives. Search hits carry no thread tags, so a placeholder for a
 * thread reply looks like a channel root; route handling must wait for the
 * real event before deciding where the message lives.
 */
export const SEARCH_HIT_PLACEHOLDER_TAG = "buzz-search-hit-placeholder";

export function isSearchHitPlaceholder(tags: readonly string[][] | undefined) {
  return (tags ?? []).some((tag) => tag[0] === SEARCH_HIT_PLACEHOLDER_TAG);
}
