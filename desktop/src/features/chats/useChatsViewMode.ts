import * as React from "react";

import {
  readChatsViewMode,
  writeChatsViewMode,
  type ChatsViewMode,
} from "./chatListGroups";

function deviceStorage(): Storage | undefined {
  return typeof window === "undefined" ? undefined : window.localStorage;
}

/** The Chats view switch, All chats | By channel, remembered per device. */
export function useChatsViewMode(): [
  ChatsViewMode,
  (mode: ChatsViewMode) => void,
] {
  const [mode, setMode] = React.useState<ChatsViewMode>(() =>
    readChatsViewMode(deviceStorage()),
  );
  const update = React.useCallback((next: ChatsViewMode) => {
    setMode(next);
    writeChatsViewMode(deviceStorage(), next);
  }, []);
  return [mode, update];
}
