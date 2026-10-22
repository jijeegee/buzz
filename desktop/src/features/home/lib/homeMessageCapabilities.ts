import { t } from "@/shared/i18n";
import type { InboxItem } from "@/features/home/lib/inbox";

export function getHomeMessageCapabilities(
  item: InboxItem | null,
  currentPubkey: string | undefined,
  availableChannelIds: ReadonlySet<string>,
) {
  const canReact = Boolean(
    item?.item.channelId && availableChannelIds.has(item.item.channelId),
  );
  const canReply =
    canReact && item?.item.kind !== 45001 && item?.item.kind !== 45003;
  const disabledReplyReason =
    canReply || !item
      ? null
      : item.item.channelId
        ? availableChannelIds.has(item.item.channelId)
          ? t("home.reply.unsupported")
          : t("home.reply.openGroup")
        : t("home.reply.noTarget");

  return {
    canDelete:
      item !== null &&
      currentPubkey?.trim().toLowerCase() ===
        item.item.pubkey.trim().toLowerCase(),
    canReact,
    canReply,
    disabledReplyReason,
  };
}
