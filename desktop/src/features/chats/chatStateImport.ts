import { markReadItemForContext } from "@/features/chats/chatStateWrites";
import {
  isChatStateAccountChanged,
  isChatStateUnavailable,
  type ChatFollowState,
  type ChatImportFollow,
  type ChatImportRead,
  type ChatStateBinding,
} from "@/shared/api/chatState";

/** Most reads plus follows per `chat_import` request (relay cap). */
export const CHAT_IMPORT_MAX_ITEMS = 500;
const FLAG_PREFIX = "kuzz.chat-state.imported.v1";
const EVENT_ID_RE = /^[0-9a-f]{64}$/i;

/** localStorage key set once every chunk of the upload was accepted. */
export function chatImportFlagKey(pubkey: string, relayUrl: string): string {
  return `${FLAG_PREFIX}:${pubkey.toLowerCase()}:${relayUrl}`;
}

export type ChatImportChunk = {
  reads: ChatImportRead[];
  follows: ChatImportFollow[];
};

export type ChatImportSources = {
  /** ReadStateManager channel and `thread:<root>` contexts (unix seconds). */
  contexts: Record<string, number>;
  followedRootIds: Iterable<string>;
  /** Muted thread roots: uploaded as unfollowed, over any follow. */
  mutedRootIds: Iterable<string>;
};

/**
 * The upload for this device's local state, in chunks of at most `maxItems`.
 * Reads skip `msg:` and malformed contexts and are capped at `nowSeconds`
 * (the relay refuses times in the future); a context named twice keeps its
 * newest time. A muted root is unfollowed even when it is also followed. A
 * thread's read and follow always share a chunk, so a thread the server has
 * no row for gets one from the same request. Every chunk with threads repeats
 * all room reads: the relay raises a thread read to its room's read only from
 * the same request (a repeated room read is skipped, but still raises).
 */
export function buildChatImportChunks(
  sources: ChatImportSources,
  nowSeconds: number,
  maxItems = CHAT_IMPORT_MAX_ITEMS,
): ChatImportChunk[] {
  const channelReads = new Map<string, ChatImportRead>();
  const threadReads = new Map<string, ChatImportRead>();
  for (const [contextId, timestamp] of Object.entries(sources.contexts)) {
    const item = markReadItemForContext(contextId, timestamp);
    if (!item || item.at === undefined) continue;
    const read = { ...item, at: Math.min(item.at, Math.floor(nowSeconds)) };
    const reads = read.kind === "channel" ? channelReads : threadReads;
    const previous = reads.get(read.id);
    if (!previous || previous.at < read.at) {
      reads.set(read.id, { kind: read.kind, id: read.id, at: read.at });
    }
  }
  const follows = new Map<string, ChatFollowState>();
  const addFollows = (roots: Iterable<string>, state: ChatFollowState) => {
    for (const root of roots) {
      if (EVENT_ID_RE.test(root)) follows.set(root.toLowerCase(), state);
    }
  };
  addFollows(sources.followedRootIds, "followed");
  addFollows(sources.mutedRootIds, "unfollowed");

  const roomReads = [...channelReads.values()];
  const threadUnits: ChatImportChunk[] = [];
  const roots = new Set([...threadReads.keys(), ...follows.keys()]);
  for (const root of roots) {
    const read = threadReads.get(root);
    const state = follows.get(root);
    threadUnits.push({
      reads: read ? [read] : [],
      follows: state ? [{ root, state }] : [],
    });
  }

  // A thread unit is at most two items; room reads go into every chunk when
  // they leave room for one, otherwise they get chunks of their own first.
  const repeatRooms =
    threadUnits.length > 0 && roomReads.length + 2 <= maxItems;
  const units: ChatImportChunk[] = repeatRooms
    ? threadUnits
    : [
        ...roomReads.map((read) => ({ reads: [read], follows: [] })),
        ...threadUnits,
      ];
  const base = repeatRooms ? roomReads.length : 0;
  const fresh = (): ChatImportChunk => ({
    reads: repeatRooms ? [...roomReads] : [],
    follows: [],
  });

  const chunks: ChatImportChunk[] = [];
  let current = fresh();
  const size = (chunk: ChatImportChunk) =>
    chunk.reads.length + chunk.follows.length;
  for (const unit of units) {
    if (size(current) > base && size(current) + size(unit) > maxItems) {
      chunks.push(current);
      current = fresh();
    }
    current.reads.push(...unit.reads);
    current.follows.push(...unit.follows);
  }
  if (size(current) > 0) chunks.push(current);
  return chunks;
}

type FlagStorage = Pick<Storage, "getItem" | "setItem">;

function defaultStorage(): FlagStorage | null {
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

export type ChatImportOutcome = "done" | "failed" | "stopped" | "unavailable";

export type ChatStateImporterDeps = {
  /** Send one chunk, bound to `binding` (refused once it is not active). */
  send: (binding: ChatStateBinding, chunk: ChatImportChunk) => Promise<unknown>;
  /** The relay answered "unavailable": turn the capability off. */
  onUnavailable: (key: string) => void;
  /** At least one chunk was stored: refetch the chats list once. */
  onImported: (key: string) => void;
  storage?: FlagStorage | null;
  nowSeconds?: () => number;
};

export type ChatStateImportInput = {
  /** Capability key (relay + pubkey), null while signed out. */
  key: string | null;
  binding: ChatStateBinding | null;
  capability: "unknown" | "on" | "off";
  /** ReadStateManager.hasMergedRelayState(): the relay markers are merged. */
  readStateInitialized: boolean;
  /** Read at start time only, after the checks above passed. */
  collect: () => ChatImportSources;
};

/**
 * Chats S4b: the first-run upload of this device's read positions and
 * follows, once per pubkey + relay.
 *
 * Runs only while the capability is on and after the local read markers are
 * complete; never while it is off or unknown. Each account is attempted at
 * most once per app start. The done flag is set only when every chunk got a
 * 200 (per-item skips are fine); otherwise the next start tries again. An
 * account or relay switch, or the capability leaving "on", stops the upload
 * before its next chunk, and the backend refuses a chunk for an account that
 * is no longer active. Contents are never logged.
 */
export class ChatStateImporter {
  private readonly deps: ChatStateImporterDeps;
  private readonly storage: FlagStorage | null;
  private readonly attempted = new Set<string>();
  private active: string | null = null;

  constructor(deps: ChatStateImporterDeps) {
    this.deps = deps;
    this.storage = deps.storage === undefined ? defaultStorage() : deps.storage;
  }

  /** Re-evaluate after any input changed; starts the upload when due. */
  update(input: ChatStateImportInput): Promise<ChatImportOutcome> | null {
    this.active = input.capability === "on" ? input.key : null;
    const { binding, key } = input;
    if (key === null || binding === null || this.active !== key) return null;
    if (!input.readStateInitialized || this.attempted.has(key)) return null;
    const flag = chatImportFlagKey(binding.pubkey, binding.relayUrl);
    if (this.readFlag(flag)) return null;
    this.attempted.add(key);
    return this.run(key, binding, flag, input.collect);
  }

  /** Signed out or torn down: stop any upload before its next chunk. */
  stop(): void {
    this.active = null;
  }

  private async run(
    key: string,
    binding: ChatStateBinding,
    flag: string,
    collect: () => ChatImportSources,
  ): Promise<ChatImportOutcome> {
    const now = this.deps.nowSeconds?.() ?? Date.now() / 1_000;
    const chunks = buildChatImportChunks(collect(), now);
    let stored = 0;
    let outcome: ChatImportOutcome = "done";
    for (const chunk of chunks) {
      if (this.active !== key) {
        outcome = "stopped";
        break;
      }
      try {
        await this.deps.send(binding, chunk);
        stored += 1;
      } catch (error) {
        if (isChatStateAccountChanged(error)) {
          outcome = "stopped";
        } else if (isChatStateUnavailable(error)) {
          outcome = "unavailable";
          this.deps.onUnavailable(key);
        } else {
          outcome = "failed";
        }
        break;
      }
    }
    if (outcome === "done") this.writeFlag(flag);
    if (stored > 0 && outcome !== "stopped" && this.active === key) {
      this.deps.onImported(key);
    }
    return outcome;
  }

  private readFlag(flag: string): boolean {
    if (!this.storage) return false;
    try {
      return this.storage.getItem(flag) !== null;
    } catch {
      return false;
    }
  }

  private writeFlag(flag: string): void {
    try {
      this.storage?.setItem(flag, "1");
    } catch {
      // Storage unavailable: the next start uploads again; the relay skips
      // what it already holds as the reader's own state.
    }
  }
}
