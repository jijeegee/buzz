import { invokeTauri } from "@/shared/api/tauri";
import type { UserSearchResult } from "@/shared/api/types";

/**
 * Friends, blocks and findability for the signed-in account. Every call acts
 * on the active community's server.
 */

/** One person found by Kuzz ID/email or in my channels. Never an email. */
export type PersonInfo = {
  pubkey: string;
  displayName: string | null;
  avatarUrl: string | null;
  username: string | null;
};

export type BlockedPerson = {
  pubkey: string;
  createdAt: string | null;
};

export type PeopleSettings = {
  /** The account's Kuzz ID, when set. */
  kuzzId: string | null;
  /** "Let people find me by ID or email". */
  discoverable: boolean;
};

/** The shape the People list and recipient picker render. */
export function personToSearchResult(person: PersonInfo): UserSearchResult {
  return {
    pubkey: person.pubkey.toLowerCase(),
    displayName: person.displayName?.trim() || person.username || null,
    avatarUrl: person.avatarUrl ?? null,
    nip05Handle: null,
    ownerPubkey: null,
    isAgent: false,
  };
}

export function getPeopleSettings(): Promise<PeopleSettings> {
  return invokeTauri<PeopleSettings>("get_people_settings");
}

/** Save (or clear, when empty) the Kuzz ID; resolves to the saved ID. */
export function setKuzzId(kuzzId: string): Promise<string | null> {
  return invokeTauri<string | null>("set_kuzz_id", { kuzzId });
}

export function setDiscoverable(discoverable: boolean): Promise<void> {
  return invokeTauri<void>("set_discoverable", { discoverable });
}

export function listBlockedPeople(): Promise<BlockedPerson[]> {
  return invokeTauri<BlockedPerson[]>("list_blocked_people");
}

export function setPersonBlocked(
  pubkey: string,
  blocked: boolean,
): Promise<void> {
  return invokeTauri<void>("set_person_blocked", {
    pubkey: pubkey.toLowerCase(),
    blocked,
  });
}

/** The one person whose Kuzz ID or email is exactly `query`, or null. */
export function lookupPerson(query: string): Promise<PersonInfo | null> {
  return invokeTauri<PersonInfo | null>("lookup_person", { query });
}

/** People in my channels whose name starts with `query`. */
export function searchSharedPeople(
  query: string,
  limit?: number,
): Promise<PersonInfo[]> {
  return invokeTauri<PersonInfo[]>("search_shared_people", {
    query,
    limit: limit ?? null,
  });
}
