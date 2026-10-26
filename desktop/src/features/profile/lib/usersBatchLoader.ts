import type { UsersBatchResponse } from "@/shared/api/types";

type Fetch = (pubkeys: string[]) => Promise<UsersBatchResponse>;

/** How long requests gather before one relay read (one render pass). */
export const USERS_BATCH_GATHER_MS = 16;
/** Pubkeys per relay read. */
export const USERS_BATCH_MAX_PUBKEYS = 200;

/**
 * Coalesces profile reads: every chat list row and timeline asks for its own
 * set of pubkeys as it renders, which at startup meant one relay read per
 * row, fetching shared members again and again. Requests made within
 * USERS_BATCH_GATHER_MS go out as one read (in chunks), and a pubkey already
 * being read is not asked for twice. Each caller gets back exactly the
 * pubkeys it asked for.
 */
export function createUsersBatchLoader(
  fetch: Fetch,
  timers: Pick<Window, "setTimeout"> = window,
) {
  const inFlight = new Map<string, Promise<UsersBatchResponse>>();
  let queued = new Set<string>();
  let flush: Promise<UsersBatchResponse> | null = null;

  const startFlush = () =>
    new Promise<UsersBatchResponse>((resolve, reject) => {
      timers.setTimeout(() => {
        const pubkeys = [...queued];
        queued = new Set();
        flush = null;
        const chunks: string[][] = [];
        for (let i = 0; i < pubkeys.length; i += USERS_BATCH_MAX_PUBKEYS) {
          chunks.push(pubkeys.slice(i, i + USERS_BATCH_MAX_PUBKEYS));
        }
        Promise.all(chunks.map((chunk) => fetch(chunk)))
          .then((responses) =>
            resolve({
              profiles: Object.assign(
                {},
                ...responses.map((response) => response.profiles),
              ),
              missing: responses.flatMap((response) => response.missing),
            }),
          )
          .catch(reject)
          .finally(() => {
            for (const pubkey of pubkeys) inFlight.delete(pubkey);
          });
      }, USERS_BATCH_GATHER_MS);
    });

  return async function load(pubkeys: string[]): Promise<UsersBatchResponse> {
    const reads = new Set<Promise<UsersBatchResponse>>();
    for (const pubkey of pubkeys) {
      let read = inFlight.get(pubkey);
      if (!read) {
        queued.add(pubkey);
        flush ??= startFlush();
        read = flush;
        inFlight.set(pubkey, read);
      }
      reads.add(read);
    }
    const responses = await Promise.all(reads);
    const profiles: UsersBatchResponse["profiles"] = {};
    const missing: string[] = [];
    for (const pubkey of pubkeys) {
      const profile = responses
        .map((response) => response.profiles[pubkey])
        .find((found) => found !== undefined);
      if (profile) profiles[pubkey] = profile;
      else missing.push(pubkey);
    }
    return { profiles, missing };
  };
}
