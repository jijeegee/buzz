import * as React from "react";

import { useBlockedPeople } from "@/features/people/BlockedPeopleContext";
import { BlockedMessageNotice } from "./BlockedMessageNotice";
import { MessageRow } from "./MessageRow";

type MessageThreadRowProps = Omit<
  React.ComponentProps<typeof MessageRow>,
  "layoutVariant"
>;

/**
 * The canonical message-row presentation used inside channel threads. A
 * blocked author's reply folds to one line until shown.
 */
export function MessageThreadRow(props: MessageThreadRowProps) {
  const { isBlocked } = useBlockedPeople();
  const [revealed, setRevealed] = React.useState(false);
  if (!revealed && isBlocked(props.message.pubkey)) {
    return <BlockedMessageNotice onShow={() => setRevealed(true)} />;
  }
  return <MessageRow {...props} layoutVariant="thread-reply" />;
}
