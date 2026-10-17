import type { ContactListResponse } from "@/shared/api/socialTypes";

/**
 * My friends live in my contact list (kind 3), which the server only stores
 * whole. Every change therefore re-reads the latest list right before writing
 * and applies exactly one add or remove, so a change made on another device
 * (or by another tab of this one) is never overwritten with an older copy.
 */

export type Contact = ContactListResponse["contacts"][number];

export type ContactChange =
  | { type: "add"; pubkeys: readonly string[] }
  | { type: "remove"; pubkey: string };

/**
 * `contacts` with `change` applied, or `null` when nothing would change (the
 * write is skipped). Other entries keep their relay hint and petname.
 */
export function applyContactChange(
  contacts: readonly Contact[],
  change: ContactChange,
): Contact[] | null {
  if (change.type === "remove") {
    const target = change.pubkey.toLowerCase();
    const kept = contacts.filter((c) => c.pubkey.toLowerCase() !== target);
    return kept.length === contacts.length ? null : kept;
  }
  const have = new Set(contacts.map((c) => c.pubkey.toLowerCase()));
  const added: Contact[] = [];
  for (const raw of change.pubkeys) {
    const pubkey = raw.toLowerCase();
    if (have.has(pubkey)) continue;
    have.add(pubkey);
    added.push({ pubkey });
  }
  return added.length === 0 ? null : [...contacts, ...added];
}

export type ContactListIo = {
  read: (owner: string) => Promise<ContactListResponse>;
  write: (contacts: Contact[]) => Promise<unknown>;
  /** Unix milliseconds; injectable for tests. */
  now?: () => number;
  sleep?: (ms: number) => Promise<void>;
};

const queues = new Map<string, Promise<unknown>>();
const lastWriteSecond = new Map<string, number>();

const defaultSleep = (ms: number) =>
  new Promise<void>((resolve) => setTimeout(resolve, ms));

/**
 * Apply one change to `owner`'s contact list: re-read, change, write. Changes
 * for one account run one at a time, and never two in the same second (the
 * list is replaced by timestamp, so a same-second write could lose to the
 * one before it). Resolves to whether a write happened.
 */
export function editContactList(
  owner: string,
  change: ContactChange,
  io: ContactListIo,
): Promise<boolean> {
  const key = owner.toLowerCase();
  const previous = queues.get(key) ?? Promise.resolve();
  const run = previous
    .catch(() => {})
    .then(async () => {
      const current = await io.read(owner);
      const next = applyContactChange(current.contacts, change);
      if (!next) return false;
      const now = io.now ?? Date.now;
      const last = lastWriteSecond.get(key);
      if (last !== undefined && Math.floor(now() / 1000) <= last) {
        await (io.sleep ?? defaultSleep)((last + 1) * 1000 - now() + 5);
      }
      await io.write(next);
      lastWriteSecond.set(key, Math.floor(now() / 1000));
      return true;
    });
  queues.set(key, run);
  const forget = () => {
    if (queues.get(key) === run) queues.delete(key);
  };
  run.then(forget, forget);
  return run;
}
