import * as React from "react";

import type { QuoteTarget } from "@/features/messages/lib/messageQuote";
import type { TimelineMessage } from "@/features/messages/types";

/**
 * One conversation surface's quote state: the channel main timeline and an
 * open thread panel each own a scope, so a Quote clicked on a row lands in the
 * composer of the same conversation the row belongs to.
 */
export type MessageQuoteScopeValue = {
  /** Start quoting a row; `null` hides the Quote action in this scope. */
  quote: ((message: TimelineMessage) => void) | null;
  /** The message the scope's composer is currently quoting. */
  target: QuoteTarget | null;
  cancel: () => void;
};

// Two contexts so rows (which only need the stable `quote` action) do not
// re-render whenever the composer's quote target changes.
const MessageQuoteActionContext = React.createContext<
  ((message: TimelineMessage) => void) | null
>(null);
const ComposerQuoteContext = React.createContext<{
  target: QuoteTarget | null;
  cancel: () => void;
} | null>(null);

export function MessageQuoteScope({
  children,
  value,
}: {
  children: React.ReactNode;
  value: MessageQuoteScopeValue;
}) {
  const { cancel, quote, target } = value;
  const composerValue = React.useMemo(
    () => ({ cancel, target }),
    [cancel, target],
  );
  return (
    <MessageQuoteActionContext.Provider value={quote}>
      <ComposerQuoteContext.Provider value={composerValue}>
        {children}
      </ComposerQuoteContext.Provider>
    </MessageQuoteActionContext.Provider>
  );
}

/** The Quote action for rows in the enclosing scope, or `null` outside one. */
export function useMessageQuoteAction() {
  return React.useContext(MessageQuoteActionContext);
}

/** The enclosing composer's quote target and cancel handler, if any. */
export function useComposerQuote() {
  return React.useContext(ComposerQuoteContext);
}
