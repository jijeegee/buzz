import * as React from "react";

/**
 * A key for the main timeline that changes when a room already on screen
 * moves where it opens (a provisional open re-anchored once the relay's
 * window is shown), so the list mounts again directly at the new position
 * instead of scrolling there (also after it sought older pages for the new
 * position, loading in between). The first decision, made before the
 * timeline was ever shown for the room, does not change it.
 */
export function useTimelineAnchorKey(
  channelId: string | null | undefined,
  anchorMessageId: string | null,
  isLoading: boolean,
): number {
  const seen = React.useRef({
    channelId,
    anchorMessageId,
    everShown: false,
    epoch: 0,
  }).current;
  if (seen.channelId !== channelId) {
    seen.channelId = channelId;
    seen.anchorMessageId = anchorMessageId;
    seen.everShown = false;
  } else if (!isLoading && seen.anchorMessageId !== anchorMessageId) {
    if (seen.everShown) seen.epoch += 1;
    seen.anchorMessageId = anchorMessageId;
  }
  if (!isLoading) seen.everShown = true;
  return seen.epoch;
}
