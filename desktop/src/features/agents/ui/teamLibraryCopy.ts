import { t } from "@/shared/i18n";
export const teamCatalogCopy = {
  get shareTitle() {
    return t("agents.share.toCatalog");
  },
  get shareDescription() {
    return t("agents.teamShare.toCatalogDescription");
  },
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
  return t("agents.teamShare.autoRetracted", { name: teamName, reason });
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
      ? t("agents.teamShare.sharingQueued", { name: teamName })
      : t("agents.teamShare.removingQueued", { name: teamName });
  }
  return shared
    ? t("agents.teamShare.published", { name: teamName })
    : t("agents.teamShare.removed", { name: teamName });
}
