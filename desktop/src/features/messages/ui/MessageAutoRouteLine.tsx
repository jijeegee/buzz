import * as React from "react";

import type { UserProfileLookup } from "@/features/profile/lib/identity";
import { UserProfilePopover } from "@/features/profile/ui/UserProfilePopover";
import { truncateNpub } from "@/shared/lib/pubkey";
import { getAutoRouteMentionPubkeys } from "../lib/agentAddressMention.mjs";

/**
 * Trailing "→ Delivered to Name" line for the agents Smart routing picked
 * (`auto-route` mention tags). Manually addressed agents render as the
 * leading address chip instead. Each name is the only label of its profile
 * trigger; the arrow is decorative.
 */
export function MessageAutoRouteLine({
  profiles,
  tags,
}: {
  profiles?: UserProfileLookup;
  tags?: string[][];
}) {
  const pubkeys = React.useMemo(() => getAutoRouteMentionPubkeys(tags), [tags]);
  if (pubkeys.length === 0) return null;
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
