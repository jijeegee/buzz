import type { Community } from "@/features/communities/types";
import { relayHttpFromWs } from "@/shared/api/inviteHelpers";
import type { WatchableAccount } from "@/shared/api/tokenAuth";

import { watchKey } from "./store";

/** The relay origin the native side keys accounts by (`origin_for`). */
export function relayOrigin(relayUrl: string): string | null {
  try {
    return relayHttpFromWs(relayUrl.trim()).replace(/\/+$/, "").toLowerCase();
  } catch {
    return null;
  }
}

export type WatchTarget = {
  key: string;
  relayUrl: string;
  account: WatchableAccount;
};

/**
 * Which watchers should run: every watchable account whose server is one of
 * this computer's communities, never the open account on the open server.
 */
export function watchTargets(
  accounts: readonly WatchableAccount[],
  communities: readonly Community[],
  open: { origin: string | null; pubkey: string | null },
): WatchTarget[] {
  const relayByOrigin = new Map<string, string>();
  for (const community of communities) {
    const origin = relayOrigin(community.relayUrl);
    if (origin && !relayByOrigin.has(origin)) {
      relayByOrigin.set(origin, community.relayUrl);
    }
  }
  const targets: WatchTarget[] = [];
  for (const account of accounts) {
    const pubkey = account.pubkey.toLowerCase();
    if (account.origin === open.origin && pubkey === open.pubkey) continue;
    const relayUrl = relayByOrigin.get(account.origin);
    if (!relayUrl) continue;
    targets.push({ key: watchKey(account.origin, pubkey), relayUrl, account });
  }
  return targets;
}
