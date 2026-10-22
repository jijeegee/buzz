import * as React from "react";

import { useBlockedPeople } from "@/features/people/BlockedPeopleContext";
import { foldsBlockedAuthors } from "@/features/people/lib/blockedDm";
import type { Channel } from "@/shared/api/types";

// Rows outside any conversation scope fold, as channels do.
const BlockedFoldContext = React.createContext(true);

/**
 * Whether rows in this conversation fold a blocked author's messages:
 * channels and group DMs do, a 1:1 DM does not.
 */
export function BlockedFoldScope({
  channel,
  children,
}: {
  channel: Pick<Channel, "channelType" | "participantPubkeys"> | null;
  children: React.ReactNode;
}) {
  return (
    <BlockedFoldContext.Provider value={foldsBlockedAuthors(channel)}>
      {children}
    </BlockedFoldContext.Provider>
  );
}

/** True when this author's message should fold to one line here. */
export function useFoldsBlockedAuthor(
  pubkey: string | null | undefined,
): boolean {
  const { isBlocked } = useBlockedPeople();
  const folds = React.useContext(BlockedFoldContext);
  return folds && isBlocked(pubkey);
}
