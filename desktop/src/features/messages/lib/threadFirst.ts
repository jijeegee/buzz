/** A room gives up waiting for its thread's relay page after this long. */
const THREAD_FIRST_MAX_WAIT_MS = 2_000;

/** The thread relay page out per room, while a thread is opening. */
const opening = new Map<string, Promise<unknown>>();

/** A thread in `channelId` asked the relay: its room's request goes after. */
export function noteThreadRequest(
  channelId: string,
  request: Promise<unknown>,
) {
  const out = request.catch(() => {});
  opening.set(channelId, out);
  void out.then(() => {
    if (opening.get(channelId) === out) opening.delete(channelId);
  });
}

/** Resolves when the room's own relay request may go out. */
export async function afterThreadRequest(channelId: string) {
  const out = opening.get(channelId);
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
