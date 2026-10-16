import type { DesktopNotificationTarget } from "@/features/notifications/lib/desktop";

import { relayOrigin } from "./targets";

/**
 * Clicking a notification for an account that is not open: switch to it,
 * then open the message. Switching restarts the app (a server change remounts
 * the shell, an account change reloads), so the target waits in
 * sessionStorage and each start takes the next step until the account is the
 * open one: server first (the native switch works on the open server), then
 * the account, then the message.
 */

const PENDING_KEY = "buzz:notification-for-account";
/** A click older than this is dropped rather than acted on at some later start. */
const PENDING_TTL_MS = 2 * 60_000;
/** Server change + account change + one spare; more means something is stuck. */
const MAX_STEPS = 3;

type NotificationAccount = NonNullable<DesktopNotificationTarget["account"]>;

type PendingTarget = {
  target: DesktopNotificationTarget & { account: NotificationAccount };
  steps: number;
  savedAt: number;
};

type PendingStorage = Pick<Storage, "getItem" | "setItem" | "removeItem">;

export type OpenForAccountDeps = {
  /** The open server origin and account (lowercase hex). */
  open: { origin: string | null; pubkey: string | null };
  /** This computer's communities, to find the one on the account's server. */
  communities: readonly { id: string; relayUrl: string }[];
  /** Whether the account is still signed in on that server here. */
  isSignedIn(account: NotificationAccount): Promise<boolean>;
  /** Open another community; the shell remounts under it. */
  switchCommunity(id: string): void;
  /** Make `pubkey` the open account and reload into it. */
  switchAccount(pubkey: string): Promise<void>;
  /** Open the message in the open account. */
  openHere(target: DesktopNotificationTarget): Promise<void>;
  /** The account cannot be opened (signed out, server gone, switch failed). */
  unavailable(reason: "signed_out" | "no_community" | "failed"): void;
  /**
   * Whether the app may move to a server origin. With a built-in server only
   * that one is allowed; a click for an account elsewhere just focuses the
   * app. Omitted = every server.
   */
  allowsServer?(origin: string): boolean;
  /** Bring the app forward without opening anything. */
  focusApp?(): void;
  storage?: PendingStorage;
  now?: () => number;
};

export function isForOpenAccount(
  target: DesktopNotificationTarget,
  open: OpenForAccountDeps["open"],
): boolean {
  const account = target.account;
  if (!account) return true;
  return (
    account.origin === open.origin &&
    account.pubkey.toLowerCase() === open.pubkey
  );
}

function storageOf(deps: OpenForAccountDeps): PendingStorage | null {
  if (deps.storage) return deps.storage;
  try {
    return window.sessionStorage;
  } catch {
    return null;
  }
}

function save(deps: OpenForAccountDeps, pending: PendingTarget): void {
  try {
    storageOf(deps)?.setItem(PENDING_KEY, JSON.stringify(pending));
  } catch {
    // Without storage the switch still happens; only the message stays closed.
  }
}

function clear(deps: OpenForAccountDeps): void {
  try {
    storageOf(deps)?.removeItem(PENDING_KEY);
  } catch {
    // Nothing to clear.
  }
}

function load(deps: OpenForAccountDeps): PendingTarget | null {
  let raw: string | null = null;
  try {
    raw = storageOf(deps)?.getItem(PENDING_KEY) ?? null;
  } catch {
    return null;
  }
  if (!raw) return null;
  try {
    const parsed = JSON.parse(raw) as PendingTarget;
    const account = parsed?.target?.account;
    if (
      typeof account?.origin !== "string" ||
      typeof account.pubkey !== "string" ||
      typeof parsed.steps !== "number" ||
      typeof parsed.savedAt !== "number"
    ) {
      return null;
    }
    return parsed;
  } catch {
    return null;
  }
}

/** Without the account, so the open shell treats it as its own. */
function withoutAccount(
  target: DesktopNotificationTarget,
): DesktopNotificationTarget {
  const { account: _account, ...rest } = target;
  return rest;
}

async function advance(
  pending: PendingTarget,
  deps: OpenForAccountDeps,
): Promise<void> {
  const { target } = pending;
  const { account } = target;
  if (isForOpenAccount(target, deps.open)) {
    clear(deps);
    await deps.openHere(withoutAccount(target));
    return;
  }
  if (pending.steps >= MAX_STEPS) {
    clear(deps);
    deps.unavailable("failed");
    return;
  }
  if (!(await deps.isSignedIn(account))) {
    clear(deps);
    deps.unavailable("signed_out");
    return;
  }
  const next = { ...pending, steps: pending.steps + 1 };
  if (
    account.origin !== deps.open.origin &&
    deps.allowsServer &&
    !deps.allowsServer(account.origin)
  ) {
    clear(deps);
    deps.focusApp?.();
    return;
  }
  if (account.origin !== deps.open.origin) {
    const community = deps.communities.find(
      (candidate) => relayOrigin(candidate.relayUrl) === account.origin,
    );
    if (!community) {
      clear(deps);
      deps.unavailable("no_community");
      return;
    }
    save(deps, next);
    deps.switchCommunity(community.id);
    return;
  }
  save(deps, next);
  try {
    await deps.switchAccount(account.pubkey);
  } catch {
    clear(deps);
    deps.unavailable("failed");
  }
}

/** A notification was clicked: open it, switching account first if needed. */
export async function openNotificationForAccount(
  target: DesktopNotificationTarget,
  deps: OpenForAccountDeps,
): Promise<void> {
  if (!target.account) {
    await deps.openHere(target);
    return;
  }
  await advance(
    {
      target: target as PendingTarget["target"],
      steps: 0,
      savedAt: (deps.now ?? Date.now)(),
    },
    deps,
  );
}

/** At start: continue a click that is switching accounts, if there is one. */
export async function resumeNotificationForAccount(
  deps: OpenForAccountDeps,
): Promise<void> {
  const pending = load(deps);
  if (!pending) {
    clear(deps);
    return;
  }
  if ((deps.now ?? Date.now)() - pending.savedAt > PENDING_TTL_MS) {
    clear(deps);
    return;
  }
  await advance(pending, deps);
}
