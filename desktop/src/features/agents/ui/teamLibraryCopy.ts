export const teamCatalogCopy = {
  chooseFromCatalog: "Choose from catalog",
  dialogTitle: "Team Catalog",
  dialogDescription: "Browse teams shared in this community.",
  emptyCatalogTitle: "No teams are being shared",
  emptyCatalogDescription: "Shared teams will appear here.",
  addAction: "Add team",
  addedAction: "Added to my teams",
  addingAction: "Adding…",
  shareTitle: "Share to catalog",
  shareDescription:
    "Anyone in this community can find and add a copy of this team. Both the team instructions and every member’s instructions are shared as plaintext. Memories and secrets aren’t included.",
} as const;

/**
 * The warning notice shown when the backend automatically queues a retraction
 * for a shared team that can no longer be projected.
 *
 * "Queued" is accurate — the tombstone has been enqueued for the flush loop
 * but the relay head may still be discoverable until the flush succeeds.
 * Using "queued for removal" rather than "was removed" avoids a false claim
 * that the catalog has already changed.
 */
export function teamAutoRetractedNotice(
  teamName: string,
  reason: string,
): string {
  return `"${teamName}" has been queued for removal from the community catalog because it can no longer be projected: ${reason}`;
}

/**
 * The result message for a share toggle.
 *
 * `queued` is not a failure: the head is durably enqueued and the flush loop
 * will publish it, so the copy promises eventual visibility rather than
 * claiming the catalog already changed.
 */
export function teamShareNotice(
  teamName: string,
  shared: boolean,
  publicationStatus: "published" | "queued",
): string {
  if (publicationStatus === "queued") {
    return shared
      ? `Sharing ${teamName}… It will appear shortly.`
      : `Removing ${teamName}… It may stay visible for a moment.`;
  }
  return shared
    ? `Published ${teamName} to the community catalog.`
    : `${teamName} is no longer discoverable in the community catalog.`;
}
