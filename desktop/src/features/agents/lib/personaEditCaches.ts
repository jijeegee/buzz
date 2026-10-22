import type { QueryClient } from "@tanstack/react-query";

/**
 * Refresh every cache a saved persona edit can invalidate.
 *
 * Shared by the plain edit mutation and the publish-on-save edit mutation so
 * the two cannot drift on what a saved edit refreshes. A preset edit changes
 * only the preset: agents made from it keep their own copy, so no agent or
 * agent-profile cache is touched.
 */
export async function invalidatePersonaEditCaches(
  queryClient: QueryClient,
): Promise<void> {
  await queryClient.invalidateQueries({ queryKey: ["personas"] });
}
