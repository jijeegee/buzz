import type * as React from "react";

import { BlockedFoldScope } from "@/features/messages/ui/blockedFoldScope";
import { ChannelPane } from "./ChannelScreenLazyViews";

export function GuardedChannelPane(
  props: React.ComponentProps<typeof ChannelPane>,
) {
  return (
    <BlockedFoldScope channel={props.activeChannel}>
      <ChannelPane {...props} />
    </BlockedFoldScope>
  );
}
