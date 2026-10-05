import * as React from "react";

import type { UserProfileLookup } from "@/features/profile/lib/identity";
import { UserProfilePopover } from "@/features/profile/ui/UserProfilePopover";
import { truncateNpub } from "@/shared/lib/pubkey";
import { getAutoRouteMentionPubkeys } from "../lib/agentAddressMention.mjs";
import { useAutoRouteStatus } from "../lib/autoRouteStatus";

const STATUS_CLASS = "mt-0.5 text-xs text-muted-foreground/80";

/**
 * Trailing "→ Delivered to Name" line for the agents Smart routing picked
 * (`auto-route` mention tags, or the local delivery before its edit echoes
 * back). Before that, a just-sent message shows "Routing…" or "Not
 * delivered" here. Manually addressed agents render as the leading address
 * chip instead. Each name is the only label of its profile trigger; the
 * arrow is decorative.
 */
export function MessageAutoRouteLine({
  messageId,
  profiles,
  tags,
}: {
  messageId?: string;
  profiles?: UserProfileLookup;
  tags?: string[][];
}) {
  const routing = useAutoRouteStatus(messageId);
  const taggedPubkeys = React.useMemo(
    () => getAutoRouteMentionPubkeys(tags),
    [tags],
  );
  const pubkeys =
    taggedPubkeys.length > 0 || routing?.status !== "delivered"
      ? taggedPubkeys
      : routing.pubkeys;
  if (pubkeys.length === 0) {
    if (routing?.status === "routing" || routing?.status === "failed") {
      return (
        <div
          className={STATUS_CLASS}
          data-testid="message-auto-route-status"
          role="status"
        >
          {routing.status === "routing" ? "Routing…" : "Not delivered"}
        </div>
      );
    }
    return null;
  }
  return (
    <div
      className="mt-0.5 flex flex-wrap items-center gap-1 text-xs text-muted-foreground/80"
      data-testid="message-auto-route"
    >
      <span aria-hidden="true">→</span>
      <span>Delivered to</span>
      {pubkeys.map((pubkey, index) => {
        const profile = profiles?.[pubkey];
        const label =
          profile?.displayName?.trim() ||
          profile?.name?.trim() ||
          truncateNpub(pubkey);
        return (
          <React.Fragment key={pubkey}>
            {index > 0 ? <span>,</span> : null}
            {/* biome-ignore lint/a11y/useValidAriaRole: UserProfilePopover uses role for agent classification, not as an ARIA attribute. */}
            <UserProfilePopover
              botIdenticonValue={label}
              pubkey={pubkey}
              role="bot"
              triggerElement="span"
            >
              <span className="font-medium hover:underline">{label}</span>
            </UserProfilePopover>
          </React.Fragment>
        );
      })}
    </div>
  );
}
