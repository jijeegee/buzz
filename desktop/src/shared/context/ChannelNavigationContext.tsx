import * as React from "react";

import type { Channel } from "@/shared/api/types";

type ChannelNavigationContextValue = {
  channels: Channel[];
  /** Names of non-DM channels, memoised once so render-hot consumers (message
   *  rows) don't each re-filter the full channel list on every render. */
  nonDmChannelNames: string[];
};

const ChannelNavigationContext =
  React.createContext<ChannelNavigationContextValue>({
    channels: [],
    nonDmChannelNames: [],
  });

function sameExceptRecency(a: Channel, b: Channel): boolean {
  if (a === b) return true;
  const keys = Object.keys(a) as (keyof Channel)[];
  if (keys.length !== Object.keys(b).length) return false;
  return keys.every(
    (key) => key === "lastMessageAt" || Object.is(a[key], b[key]),
  );
}

/**
 * Every live message rewrites one channel's `lastMessageAt`, which no
 * navigation consumer (names, links, avatars) reads. Keeping the previous list
 * for recency-only changes stops each message row in the open room from
 * re-rendering on other rooms' events. Read recency from the channels query.
 */
function useNavigationChannels(channels: Channel[]): Channel[] {
  const ref = React.useRef(channels);
  const previous = ref.current;
  if (
    previous !== channels &&
    previous.length === channels.length &&
    previous.every((channel, index) =>
      sameExceptRecency(channel, channels[index]),
    )
  ) {
    return previous;
  }
  ref.current = channels;
  return channels;
}

export function ChannelNavigationProvider({
  channels,
  children,
}: {
  channels: Channel[];
  children: React.ReactNode;
}) {
  const navigationChannels = useNavigationChannels(channels);
  const value = React.useMemo(
    () => ({
      channels: navigationChannels,
      nonDmChannelNames: navigationChannels
        .filter((c) => c.channelType !== "dm")
        .map((c) => c.name),
    }),
    [navigationChannels],
  );

  return (
    <ChannelNavigationContext.Provider value={value}>
      {children}
    </ChannelNavigationContext.Provider>
  );
}

export function useChannelNavigation() {
  return React.useContext(ChannelNavigationContext);
}
