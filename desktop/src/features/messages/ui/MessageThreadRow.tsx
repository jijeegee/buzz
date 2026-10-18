import * as React from "react";

import { BlockedMessageNotice } from "./BlockedMessageNotice";
import { useFoldsBlockedAuthor } from "./blockedFoldScope";
import { MessageRow } from "./MessageRow";

type MessageThreadRowProps = Omit<
  React.ComponentProps<typeof MessageRow>,
  "layoutVariant"
>;

/**
 * The canonical message-row presentation used inside channel threads. A
 * blocked author's reply folds to one line until shown (not in a 1:1 DM).
 */
export function MessageThreadRow(props: MessageThreadRowProps) {
  const foldsAuthor = useFoldsBlockedAuthor(props.message.pubkey);
  const [revealed, setRevealed] = React.useState(false);
  if (!revealed && foldsAuthor) {
    return <BlockedMessageNotice onShow={() => setRevealed(true)} />;
  }
  return <MessageRow {...props} layoutVariant="thread-reply" />;
}
