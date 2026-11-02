import * as React from "react";

type Resolved = { channelId: string; key: string };

/**
 * Whether a channel route's message target (`messageId`, with the thread it
 * is in) is still being fetched. The settled key is kept with the channel it
 * settled in and dropped when the target clears, so a later open of the same
 * target, in another channel or after the panel closed, waits for its own
 * fetch instead of reusing an old one. Derived during render, so the first
 * render after a change is already right.
 */
export function useRouteTargetResolution(
  channelId: string,
  targetKey: string | null,
) {
  const [resolved, setResolved] = React.useState<Resolved | null>(null);
  if (targetKey === null && resolved !== null) {
    setResolved(null);
  }
  const isResolving =
    targetKey !== null &&
    !(resolved?.channelId === channelId && resolved.key === targetKey);
  const markResolved = React.useCallback(
    (settledChannelId: string, key: string) =>
      setResolved({ channelId: settledChannelId, key }),
    [],
  );
  return { isResolving, markResolved };
}

/** The key of a route's message target; thread-only routes have none. */
export function routeTargetKeyFor(
  targetMessageId: string | null,
  targetThreadRootId: string | null,
): string {
  return `${targetMessageId ?? ""}|${targetThreadRootId ?? ""}`;
}
