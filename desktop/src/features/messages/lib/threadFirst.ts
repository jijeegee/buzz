import type { QueryClient } from "@tanstack/react-query";

/** A room gives up waiting for its thread's relay page after this long. */
const THREAD_FIRST_MAX_WAIT_MS = 2_000;

/** The relay page out per room, while a thread opens. */
const opening = new WeakMap<QueryClient, Map<string, Promise<unknown>>>();

/** A thread in `channelId` is opening: its room's request goes after it. */
export function noteThreadRequest(
  queryClient: QueryClient,
  channelId: string,
  request: Promise<unknown>,
) {
  let rooms = opening.get(queryClient);
  if (!rooms) {
    rooms = new Map();
    opening.set(queryClient, rooms);
  }
  const out = request.catch(() => {});
  rooms.set(channelId, out);
  void out.then(() => {
    if (rooms.get(channelId) === out) rooms.delete(channelId);
  });
}

/** Resolves when the room's own relay request may go out. */
export async function afterThreadRequest(
  queryClient: QueryClient,
  channelId: string,
) {
  const out = opening.get(queryClient)?.get(channelId);
  if (!out) return;
  let timer: ReturnType<typeof setTimeout> | undefined;
  await Promise.race([
    out,
    new Promise((resolve) => {
      timer = setTimeout(resolve, THREAD_FIRST_MAX_WAIT_MS);
    }),
  ]);
  clearTimeout(timer);
}
