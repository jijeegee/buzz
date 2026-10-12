import * as React from "react";

let channelRouteScreenModule: Promise<
  typeof import("./ChannelRouteScreen")
> | null = null;

function loadChannelRouteScreen() {
  channelRouteScreenModule ??= import("./ChannelRouteScreen");
  return channelRouteScreenModule;
}

export const LazyChannelRouteScreen = React.lazy(async () => {
  const module = await loadChannelRouteScreen();
  return { default: module.ChannelRouteScreen };
});

/**
 * Load the chat screen's code ahead of the first room open, so opening a room
 * right after launch does not wait on the chunk behind a loading screen.
 */
export function preloadChannelRouteScreen(): void {
  void loadChannelRouteScreen().catch(() => {
    // A failed preload is retried by the route's own lazy load.
    channelRouteScreenModule = null;
  });
}
