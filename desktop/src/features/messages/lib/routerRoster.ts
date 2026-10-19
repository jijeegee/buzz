import type { RouterRosterEntry } from "@/shared/api/tauriMessageRouting";
import { normalizePubkey } from "@/shared/lib/pubkey";

/**
 * Builds Smart routing's roster: the agent members of the channel the
 * router may pick, each with the best capability text available. Pure, so
 * the description priority and caps are table-tested; Rust re-enforces the
 * caps before anything reaches the model.
 *
 * Description priority (first non-blank wins):
 * 1. my managed agent's own `description`;
 * 2. the agent's kind:0 `about` (the same public text for anyone's agent);
 * 3. the first {@link SYSTEM_PROMPT_PREFIX_CHARS} characters of my own
 *    agent's system prompt (never someone else's — it is private);
 * 4. nothing (name only).
 */

export const MAX_ROUTER_ROSTER = 24;
export const MAX_ROUTER_DESCRIPTION_CHARS = 200;
export const SYSTEM_PROMPT_PREFIX_CHARS = 160;
export const MAX_ROUTER_HUMANS = 8;

export type RosterIdentity = {
  pubkey: string;
  label: string;
  isAgent: boolean;
};

export type RosterManagedAgent = {
  pubkey: string;
  description: string | null;
  systemPrompt: string | null;
};

function clip(text: string, max: number): string {
  return Array.from(text).slice(0, max).join("");
}

function nonBlank(text: string | null | undefined): string | null {
  const trimmed = text?.replace(/\s+/g, " ").trim() ?? "";
  return trimmed.length > 0 ? trimmed : null;
}

export function buildRouterRoster({
  aboutByPubkey,
  identities,
  managedAgents,
  memberPubkeys,
  selfPubkey,
}: {
  /** kind:0 `about` per normalized pubkey, for whatever is loaded. */
  aboutByPubkey: ReadonlyMap<string, string | null>;
  identities: readonly RosterIdentity[];
  managedAgents: readonly RosterManagedAgent[];
  /** Normalized pubkeys of the channel's members. */
  memberPubkeys: ReadonlySet<string>;
  selfPubkey: string | null;
}): RouterRosterEntry[] {
  const self = selfPubkey ? normalizePubkey(selfPubkey) : null;
  const managedByPubkey = new Map(
    managedAgents.map((agent) => [normalizePubkey(agent.pubkey), agent]),
  );
  const seen = new Set<string>();
  const roster: RouterRosterEntry[] = [];
  for (const identity of identities) {
    const pubkey = normalizePubkey(identity.pubkey);
    const name = nonBlank(identity.label);
    if (
      !identity.isAgent ||
      !name ||
      pubkey === self ||
      !memberPubkeys.has(pubkey) ||
      seen.has(pubkey)
    ) {
      continue;
    }
    seen.add(pubkey);
    const managed = managedByPubkey.get(pubkey);
    const ownPrompt = nonBlank(managed?.systemPrompt);
    const description =
      nonBlank(managed?.description) ??
      nonBlank(aboutByPubkey.get(pubkey)) ??
      (managed && ownPrompt
        ? clip(ownPrompt, SYSTEM_PROMPT_PREFIX_CHARS)
        : null);
    roster.push({
      pubkey,
      name,
      description: description
        ? clip(description, MAX_ROUTER_DESCRIPTION_CHARS)
        : null,
    });
  }
  return roster
    .sort((a, b) => a.pubkey.localeCompare(b.pubkey))
    .slice(0, MAX_ROUTER_ROSTER);
}

/** Display names of the channel's human members (for the HUMANS line). */
export function routerHumanNames(
  identities: readonly RosterIdentity[],
  memberPubkeys: ReadonlySet<string>,
): string[] {
  const names: string[] = [];
  for (const identity of identities) {
    const name = nonBlank(identity.label);
    if (
      identity.isAgent ||
      !name ||
      !memberPubkeys.has(normalizePubkey(identity.pubkey)) ||
      names.includes(name)
    ) {
      continue;
    }
    names.push(name);
    if (names.length === MAX_ROUTER_HUMANS) break;
  }
  return names;
}
