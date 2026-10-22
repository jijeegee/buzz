import { Ban } from "lucide-react";
import { toast } from "sonner";

import { useUnblockPersonMutation } from "@/features/people/BlockedPeopleContext";
import {
  resolveKnownUserLabel,
  type UserProfileLookup,
} from "@/features/profile/lib/identity";
import type { Channel } from "@/shared/api/types";
import { Button } from "@/shared/ui/button";
import { t as translate, useT } from "@/shared/i18n";

/** The banner's line: the person's name when known, never their key. */
export function blockedDmBannerText(name: string | null): string {
  return name
    ? translate("channels.blockedBanner.named", { name })
    : translate("channels.blockedBanner.unnamed");
}

/**
 * Docked to the top of the (disabled) composer in a 1:1 DM with someone I
 * blocked. Unblock is the same call as Settings › Blocked users; the block
 * list updates at once, so the banner leaves and the composer opens.
 */
export function BlockedDmComposerBanner({
  channel,
  currentPubkey,
  peer,
  profiles,
}: {
  channel: Pick<Channel, "participantPubkeys" | "participants">;
  currentPubkey: string | undefined;
  peer: string;
  profiles?: UserProfileLookup;
}) {
  const t = useT();
  const unblock = useUnblockPersonMutation(currentPubkey);
  const index = channel.participantPubkeys.findIndex(
    (pubkey) => pubkey.toLowerCase() === peer,
  );
  const name = resolveKnownUserLabel({
    currentPubkey,
    fallbackName: index >= 0 ? channel.participants[index] : null,
    profiles,
    pubkey: peer,
  });
  return (
    <div
      className="relative z-0 mx-5 -mb-3 flex items-center gap-2 rounded-t-2xl border border-b-0 border-border/80 bg-muted/80 px-4 pb-5 pt-2 text-sm leading-5 text-foreground backdrop-blur-sm"
      data-testid="composer-blocked-banner"
    >
      <Ban aria-hidden className="h-4 w-4 shrink-0 text-muted-foreground" />
      <span className="min-w-0 flex-1 truncate">
        {blockedDmBannerText(name)}
      </span>
      <Button
        data-testid="composer-blocked-unblock"
        disabled={unblock.isPending}
        onClick={() =>
          unblock.mutate(peer, {
            onError: () =>
              toast.error(t("channels.blockedBanner.unblockFailed")),
          })
        }
        size="sm"
        type="button"
        variant="outline"
      >
        {t("channels.blockedBanner.unblock")}
      </Button>
    </div>
  );
}
