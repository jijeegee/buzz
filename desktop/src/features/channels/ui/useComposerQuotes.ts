import * as React from "react";

import {
  type QuoteTarget,
  quoteTargetFromMessage,
  withQuoteTags,
} from "@/features/messages/lib/messageQuote";
import type { MessageQuoteScopeValue } from "@/features/messages/ui/messageQuoteScope";
import type { TimelineMessage } from "@/features/messages/types";

type ComposerSend<Result> = (
  content: string,
  mentionPubkeys: string[],
  mediaTags?: string[][],
  channelId?: string | null,
  threadContext?: {
    parentEventId: string | null;
    threadHeadId: string | null;
  } | null,
  forceRest?: boolean,
) => Promise<Result>;

/** A quote is bound to the conversation it was started in. */
type ScopedQuote = { scopeKey: string; target: QuoteTarget };

/**
 * One quote scope (main timeline or open thread). The quote only applies
 * while `scopeKey` still names the conversation it was started in, so
 * switching channels or threads silently drops it. `send` is wrapped so the
 * NIP-18 `q` tag rides the outgoing tag set; placement is left entirely to
 * the wrapped sender (top-level for main, the usual e-tags in a thread).
 */
function useQuoteScope<Result>(
  scopeKey: string | null,
  enabled: boolean,
  send: ComposerSend<Result>,
): { scope: MessageQuoteScopeValue; send: ComposerSend<Result> } {
  const [quote, setQuote] = React.useState<ScopedQuote | null>(null);
  const target =
    enabled && quote && scopeKey !== null && quote.scopeKey === scopeKey
      ? quote.target
      : null;
  const targetRef = React.useRef(target);
  targetRef.current = target;

  const startQuote = React.useCallback(
    (message: TimelineMessage) => {
      const next = quoteTargetFromMessage(message);
      if (next && scopeKey !== null) setQuote({ scopeKey, target: next });
    },
    [scopeKey],
  );
  const cancel = React.useCallback(() => setQuote(null), []);

  const wrappedSend = React.useCallback<ComposerSend<Result>>(
    async (content, mentionPubkeys, mediaTags, ...rest) => {
      const captured = targetRef.current;
      // A submit that fans out into several sends quotes only once.
      targetRef.current = null;
      let sent: Result;
      try {
        sent = await send(
          content,
          mentionPubkeys,
          withQuoteTags(mediaTags, captured),
          ...rest,
        );
      } catch (error) {
        // Keep the quote for a retry unless the user changed it meanwhile.
        targetRef.current ??= captured;
        throw error;
      }
      if (captured) {
        // Clear only the quote this send consumed; a newer pick survives.
        setQuote((current) => (current?.target === captured ? null : current));
      }
      return sent;
    },
    [send],
  );

  const scope = React.useMemo<MessageQuoteScopeValue>(
    () => ({
      cancel,
      quote: enabled && scopeKey !== null ? startQuote : null,
      target,
    }),
    [cancel, enabled, scopeKey, startQuote, target],
  );
  return { scope, send: wrappedSend };
}

export function useComposerQuotes<MainResult, ThreadResult>({
  activeChannelId,
  enabled,
  sendMain,
  sendThread,
  threadHeadId,
}: {
  activeChannelId: string | null;
  enabled: boolean;
  sendMain: ComposerSend<MainResult>;
  sendThread: ComposerSend<ThreadResult>;
  threadHeadId: string | null;
}) {
  const main = useQuoteScope(activeChannelId, enabled, sendMain);
  const thread = useQuoteScope(
    activeChannelId && threadHeadId
      ? `${activeChannelId}:${threadHeadId}`
      : null,
    enabled,
    sendThread,
  );
  return {
    mainScope: main.scope,
    sendMain: main.send,
    sendThread: thread.send,
    threadScope: thread.scope,
  };
}
