import type {
  KnownProfile,
  UserProfileSummary,
  UsersBatchResponse,
} from "@/shared/api/types";

/** `known`: the profiles already held, per pubkey. */
type Fetch = (
  pubkeys: string[],
  known: Record<string, KnownProfile>,
) => Promise<UsersBatchResponse>;

/**
 * A loader answer. `kept` holds, for each `unchanged` pubkey, the held
 * profile the read was asked about (whichever asker held it), so an asker
 * holding nothing itself still gets it.
 */
export type UsersBatchLoad = UsersBatchResponse & {
  kept: Record<string, UserProfileSummary>;
};

/** How long requests gather before one relay read (one render pass). */
export const USERS_BATCH_GATHER_MS = 16;
/** Pubkeys per relay read. */
export const USERS_BATCH_MAX_PUBKEYS = 200;

let communityGeneration = 0;

/** Reject queued and in-flight profile reads before changing communities. */
export function resetUsersBatchLoaders(): void {
  communityGeneration += 1;
}

type Gathered = { known: KnownProfile; summary: UserProfileSummary };

/**
 * Whether held profile `first` is replaced by `second` on the relay: older
 * `createdAt`, or the same second with a higher event id (lowest id wins).
 */
function replacedBy(first: KnownProfile, second: KnownProfile): boolean {
  return (
    first.createdAt < second.createdAt ||
    (first.createdAt === second.createdAt && first.eventId > second.eventId)
  );
}

/**
 * Coalesces profile reads: every chat list row and timeline asks for its own
 * set of pubkeys as it renders, which at startup meant one relay read per
 * row, fetching shared members again and again. Requests made within
 * USERS_BATCH_GATHER_MS go out as one read (in chunks), and a pubkey already
 * being read is not asked for twice. Each caller gets back exactly the
 * pubkeys it asked for.
 *
 * A caller passes the profiles it already holds (`held`); those are asked
 * only for what changed since their last check and come back `unchanged`
 * when nothing did. A pubkey gathered from several callers is asked in full
 * when any of them holds nothing for it, and otherwise about the oldest
 * profile held, checked at the earliest time, so the answer covers all.
 */
export function createUsersBatchLoader(
  fetch: Fetch,
  timers: Pick<Window, "setTimeout"> = window,
) {
  const generation = communityGeneration;
  let disposed = false;
  const isActive = () => !disposed && generation === communityGeneration;
  const assertActive = () => {
    if (!isActive()) throw new Error("Profile read invalidated");
  };
  const inFlight = new Map<string, Promise<UsersBatchLoad>>();
  /** Gathered pubkeys and what every asker holds (undefined: not all do). */
  let queued = new Map<string, Gathered | undefined>();
  let flush: Promise<UsersBatchLoad> | null = null;

  const startFlush = () =>
    new Promise<UsersBatchLoad>((resolve, reject) => {
      timers.setTimeout(() => {
        const gathered = queued;
        const pubkeys = [...gathered.keys()];
        queued = new Map();
        flush = null;
        const chunks: string[][] = [];
        for (let i = 0; i < pubkeys.length; i += USERS_BATCH_MAX_PUBKEYS) {
          chunks.push(pubkeys.slice(i, i + USERS_BATCH_MAX_PUBKEYS));
        }
        const knownOf = (chunk: string[]) => {
          const known: Record<string, KnownProfile> = {};
          for (const pubkey of chunk) {
            const held = gathered.get(pubkey);
            if (held !== undefined) known[pubkey] = held.known;
          }
          return known;
        };
        Promise.resolve()
          .then(() => {
            assertActive();
            return Promise.all(
              chunks.map((chunk) => fetch(chunk, knownOf(chunk))),
            );
          })
          .then((responses) => {
            assertActive();
            const unchanged = responses.flatMap(
              (response) => response.unchanged ?? [],
            );
            const kept: UsersBatchLoad["kept"] = {};
            for (const pubkey of unchanged) {
              const held = gathered.get(pubkey);
              if (held) kept[pubkey] = held.summary;
            }
            resolve({
              profiles: Object.assign(
                {},
                ...responses.map((response) => response.profiles),
              ),
              missing: responses.flatMap((response) => response.missing),
              unchanged,
              kept,
            });
          })
          .catch(reject)
          .finally(() => {
            for (const pubkey of pubkeys) inFlight.delete(pubkey);
          });
      }, USERS_BATCH_GATHER_MS);
    });

  async function load(
    pubkeys: string[],
    held: Record<string, UserProfileSummary> = {},
  ): Promise<UsersBatchLoad> {
    assertActive();
    const known = knownProfiles(held);
    const reads = new Set<Promise<UsersBatchLoad>>();
    for (const pubkey of pubkeys) {
      let read = inFlight.get(pubkey);
      const mine: Gathered | undefined = known[pubkey]
        ? { known: known[pubkey], summary: held[pubkey] }
        : undefined;
      if (read && queued.has(pubkey)) {
        const gathered = queued.get(pubkey);
        if (gathered === undefined || mine === undefined) {
          queued.set(pubkey, undefined);
        } else {
          const older = replacedBy(mine.known, gathered.known)
            ? mine
            : gathered;
          queued.set(pubkey, {
            summary: older.summary,
            known: {
              ...older.known,
              verifiedAt: Math.min(
                mine.known.verifiedAt,
                gathered.known.verifiedAt,
              ),
            },
          });
        }
      }
      if (!read) {
        queued.set(pubkey, mine);
        flush ??= startFlush();
        read = flush;
        inFlight.set(pubkey, read);
      }
      reads.add(read);
    }
    const responses = await Promise.all(reads);
    assertActive();
    const kept: UsersBatchLoad["kept"] = Object.assign(
      {},
      ...responses.map((response) => response.kept),
    );
    const profiles: UsersBatchResponse["profiles"] = {};
    const missing: string[] = [];
    const unchanged: string[] = [];
    const keptForMe: UsersBatchLoad["kept"] = {};
    for (const pubkey of pubkeys) {
      const profile = responses
        .map((response) => response.profiles[pubkey])
        .find((found) => found !== undefined);
      if (profile) profiles[pubkey] = profile;
      else if (kept[pubkey]) {
        unchanged.push(pubkey);
        keptForMe[pubkey] = kept[pubkey];
      } else missing.push(pubkey);
    }
    return { profiles, missing, unchanged, kept: keptForMe };
  }

  return Object.assign(load, {
    isActive,
    assertActive,
    dispose: () => {
      disposed = true;
    },
  });
}

/**
 * The held profile the relay would keep: newer `createdAt`, then the lower
 * event id, then the later check. One without them is older.
 */
export function newerHeldProfile(
  first: UserProfileSummary | undefined,
  second: UserProfileSummary | undefined,
): UserProfileSummary | undefined {
  if (!first || !second) return first ?? second;
  const firstCreated = first.createdAt ?? -1;
  const secondCreated = second.createdAt ?? -1;
  if (firstCreated !== secondCreated) {
    return secondCreated > firstCreated ? second : first;
  }
  const firstId = first.eventId ?? "";
  const secondId = second.eventId ?? "";
  if (firstId !== secondId) {
    if (!firstId || !secondId) return firstId ? first : second;
    return secondId < firstId ? second : first;
  }
  return (second.verifiedAt ?? -1) > (first.verifiedAt ?? -1) ? second : first;
}

/**
 * The held profiles a read can ask only for changes: those carrying
 * `createdAt`, `verifiedAt` and `eventId`. Profiles saved by earlier builds
 * lack them and are asked for in full.
 */
export function knownProfiles(
  held: Record<string, UserProfileSummary>,
): Record<string, KnownProfile> {
  const known: Record<string, KnownProfile> = {};
  for (const [pubkey, { createdAt, verifiedAt, eventId }] of Object.entries(
    held,
  )) {
    if (
      typeof createdAt === "number" &&
      typeof verifiedAt === "number" &&
      typeof eventId === "string"
    ) {
      known[pubkey] = { createdAt, verifiedAt, eventId };
    }
  }
  return known;
}

/**
 * A batch answer applied to the profiles held before it: a returned profile
 * replaces the held one, an unchanged pubkey keeps the profile the read was
 * asked about (`kept`, possibly another asker's), and only pubkeys nothing
 * was held for can be missing. Both count as checked at `verifiedAt`, taken
 * before the read was asked.
 */
export function applyUsersBatchAnswer(
  pubkeys: string[],
  held: Record<string, UserProfileSummary>,
  answer: UsersBatchResponse & { kept?: UsersBatchLoad["kept"] },
  verifiedAt: number,
): UsersBatchResponse {
  const unchanged = new Set(answer.unchanged ?? []);
  const missing = new Set(answer.missing);
  const result: UsersBatchResponse = { profiles: {}, missing: [] };
  for (const pubkey of pubkeys) {
    const summary =
      answer.profiles[pubkey] ??
      (unchanged.has(pubkey)
        ? (answer.kept?.[pubkey] ?? held[pubkey])
        : undefined);
    if (summary) result.profiles[pubkey] = { ...summary, verifiedAt };
    else if (missing.has(pubkey)) result.missing.push(pubkey);
  }
  return result;
}
