import { relayHttpFromWs } from "@/shared/api/inviteHelpers";
import { KUZZ_SEQ_EXTENSION } from "@/shared/api/relaySeqCursor";

const PROBE_TIMEOUT_MS = 3_000;
const lastKnown = new Map<string, boolean>();

export type KuzzSeqProbe = (relayUrl: string) => Promise<boolean>;

async function fetchSupportsKuzzSeq(base: string) {
  try {
    const response = await fetch(`${base}/info`, {
      headers: { Accept: "application/nostr+json" },
      signal: AbortSignal.timeout(PROBE_TIMEOUT_MS),
    });
    if (!response.ok) return lastKnown.get(base) ?? false;
    const info = (await response.json()) as {
      supported_extensions?: unknown;
    };
    const supported =
      Array.isArray(info.supported_extensions) &&
      info.supported_extensions.includes(KUZZ_SEQ_EXTENSION);
    lastKnown.set(base, supported);
    return supported;
  } catch {
    return lastKnown.get(base) ?? false;
  }
}

/**
 * Whether the relay at `relayUrl` advertises `kuzz-seq` in NIP-11. The first
 * connect to an address waits for the answer; later connects use the last
 * answer at once and refresh it in the background, so a server upgrade or
 * rollback is followed by the next reconnect. A failed probe keeps the last
 * answer (false before the first), so an old server is never sent `after_seq`.
 */
export const probeKuzzSeq: KuzzSeqProbe = async (relayUrl) => {
  let base: string;
  try {
    base = relayHttpFromWs(relayUrl).replace(/\/+$/, "");
  } catch {
    return false;
  }
  const known = lastKnown.get(base);
  if (known === undefined) return fetchSupportsKuzzSeq(base);
  void fetchSupportsKuzzSeq(base);
  return known;
};
