import type * as React from "react";

import { isChatRowStandIn } from "@/features/chats/chatRowChannel";
import { BlockedFoldScope } from "@/features/messages/ui/blockedFoldScope";
import { ChannelPane } from "./ChannelScreenLazyViews";

/**
 * A room opened from its Chats row shows its messages, but takes no sends
 * until the real channel arrives (docs/message-sync.md#launch-snapshots).
 * The space stays blank: no loading text while the room is already readable.
 */
function StandInFooter() {
  return (
    <div
      aria-hidden="true"
      className="h-14"
      data-testid="chat-row-stand-in-footer"
    />
  );
}

export function GuardedChannelPane(
  props: React.ComponentProps<typeof ChannelPane>,
) {
  const readOnlyFooter =
    props.readOnlyFooter ??
    (isChatRowStandIn(props.activeChannel) ? <StandInFooter /> : undefined);
  return (
    <BlockedFoldScope channel={props.activeChannel}>
      <ChannelPane {...props} readOnlyFooter={readOnlyFooter} />
    </BlockedFoldScope>
  );
}
