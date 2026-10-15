import { mentionOccurrences } from "@/shared/lib/mentionOccurrences";

export type MentionPubkeyCandidate = {
  displayName: string | null;
  isMember: boolean;
  pubkey?: string;
};

type MentionMatch = {
  displayName: string;
  pubkey?: string;
};

function normalizeDisplayName(name: string): string {
  return name.trim().toLowerCase();
}

/** A later selection of a label already bound to a different identity. */
export type RepeatedMention = {
  displayName: string;
  pubkey: string;
  isAgent?: boolean;
};

function boundPubkeyForLabel(
  displayName: string,
  selectedMentions: ReadonlyMap<string, string>,
): string | undefined {
  const label = normalizeDisplayName(displayName);
  for (const [name, pubkey] of selectedMentions) {
    if (normalizeDisplayName(name) === label) return pubkey;
  }
  return undefined;
}

/**
 * Record a picked identity under its draft label. Draft text never carries key
 * material, so a label already bound to another identity keeps that binding
 * and queues this identity in selection order: two different Scouts are both
 * written `@Scout`, and the Nth `@Scout` occurrence binds the Nth pick.
 * Returns whether the label's primary binding was written.
 */
export function bindSelectedMention(
  selectedMentions: Map<string, string>,
  repeatedMentions: RepeatedMention[],
  displayName: string,
  pubkey: string,
  isAgent?: boolean,
): boolean {
  const existing = boundPubkeyForLabel(displayName, selectedMentions);
  if (
    existing !== undefined &&
    existing.toLowerCase() !== pubkey.toLowerCase()
  ) {
    const key = pubkey.toLowerCase();
    const label = normalizeDisplayName(displayName);
    if (
      !repeatedMentions.some(
        (repeated) =>
          repeated.pubkey.toLowerCase() === key &&
          normalizeDisplayName(repeated.displayName) === label,
      )
    ) {
      repeatedMentions.push(
        isAgent === undefined
          ? { displayName, pubkey }
          : { displayName, pubkey, isAgent },
      );
    }
    return false;
  }
  selectedMentions.set(displayName, pubkey);
  return true;
}

/**
 * Before a new pick of `displayName`, forget its bindings whose text is gone.
 * Occurrences bind to picks in selection order, so with N occurrences left in
 * `text` only the first N picks are still present; keeping a deleted pick
 * would hand its slot to whoever is picked next.
 */
export function pruneSelectedMentions(
  selectedMentions: Map<string, string>,
  repeatedMentions: RepeatedMention[],
  displayName: string,
  text: string,
): void {
  const label = normalizeDisplayName(displayName);
  const primary = [...selectedMentions].find(
    ([name]) => normalizeDisplayName(name) === label,
  );
  if (!primary) return;
  const queued = repeatedMentions.filter(
    (repeated) => normalizeDisplayName(repeated.displayName) === label,
  );
  const keep = mentionOccurrences(text, [{ displayName: primary[0] }]).length;
  if (1 + queued.length <= keep) return;
  const kept = [
    { displayName: primary[0], pubkey: primary[1] } as RepeatedMention,
    ...queued,
  ].slice(0, keep);
  selectedMentions.delete(primary[0]);
  for (const repeated of queued) {
    repeatedMentions.splice(repeatedMentions.indexOf(repeated), 1);
  }
  const [first, ...rest] = kept;
  if (first) selectedMentions.set(first.displayName, first.pubkey);
  repeatedMentions.push(...rest);
}

/**
 * Drop one identity's binding. When it held a label's primary binding, the
 * next identity queued under that label is promoted so the remaining
 * occurrences keep binding in selection order.
 */
export function releaseSelectedMention(
  selectedMentions: Map<string, string>,
  repeatedMentions: RepeatedMention[],
  pubkey: string,
): void {
  const key = pubkey.toLowerCase();
  for (let index = repeatedMentions.length - 1; index >= 0; index--) {
    if (repeatedMentions[index].pubkey.toLowerCase() === key) {
      repeatedMentions.splice(index, 1);
    }
  }
  for (const [label, bound] of [...selectedMentions]) {
    if (bound.toLowerCase() !== key) continue;
    selectedMentions.delete(label);
    const next = repeatedMentions.findIndex(
      (repeated) =>
        normalizeDisplayName(repeated.displayName) ===
        normalizeDisplayName(label),
    );
    if (next >= 0) {
      const [promoted] = repeatedMentions.splice(next, 1);
      selectedMentions.set(label, promoted.pubkey);
    }
  }
}

/**
 * The draft label for a picked identity. Always the plain display name: a
 * same-name collision is resolved by `bindSelectedMention`'s ordered binding,
 * never by appending key material to the visible text.
 */
export function selectedMentionLabel(
  displayName: string,
  _pubkey: string,
  _selectedMentions: ReadonlyMap<string, string>,
): string {
  return displayName;
}

/** Labels for a multi-identity selection (e.g. a team); see `selectedMentionLabel`. */
export function selectedMentionLabels<
  T extends { displayName: string; pubkey?: string },
>(
  selections: readonly T[],
  _selectedMentions: ReadonlyMap<string, string>,
): T[] {
  return [...selections];
}

/**
 * Build shared candidates for selected bindings, personas and typed members.
 * Selected labels suppress same-name member fallback; unbound selected labels
 * still compete with shorter keyed labels for exact occurrence ownership.
 */
export function mentionMatchCandidates({
  selectedMentions,
  selectedDisplayNames,
  memberCandidates,
  competingDisplayNames,
}: {
  selectedMentions: ReadonlyMap<string, string>;
  selectedDisplayNames?: Iterable<string>;
  competingDisplayNames?: Iterable<string>;
  memberCandidates: readonly MentionPubkeyCandidate[];
}): MentionMatch[] {
  const selectedLabels = [...(selectedDisplayNames ?? [])];
  const selectedNames = new Set(
    [...selectedMentions.keys(), ...selectedLabels].map(normalizeDisplayName),
  );
  const candidates: MentionMatch[] = [];

  const addMatches = (displayName: string, pubkey?: string) => {
    const trimmedName = displayName.trim();
    if (!trimmedName) return;

    candidates.push({ displayName: trimmedName, pubkey });
  };

  for (const [displayName, pubkey] of selectedMentions) {
    addMatches(displayName, pubkey);
  }
  for (const displayName of selectedLabels) {
    addMatches(displayName);
  }
  for (const candidate of memberCandidates) {
    if (
      candidate.pubkey &&
      candidate.isMember &&
      candidate.displayName &&
      !selectedNames.has(normalizeDisplayName(candidate.displayName))
    ) {
      addMatches(candidate.displayName, candidate.pubkey);
    }
  }

  // Historical unresolved labels own ranges without binding or suppressing
  // current same-name members. They must also block shorter selected labels.
  for (const displayName of competingDisplayNames ?? []) {
    addMatches(displayName);
  }
  return candidates;
}

/**
 * Extract recipients from the same exact occurrences used by draft routing.
 * Occurrences of a label picked for several identities bind, in text order,
 * to the label's primary selection and then to `repeatedMentions` in selection
 * order, so same-name recipients share one keyless body notation.
 */
export function extractMentionPubkeys(options: {
  text: string;
  selectedMentions: ReadonlyMap<string, string>;
  selectedDisplayNames?: Iterable<string>;
  competingDisplayNames?: Iterable<string>;
  memberCandidates: readonly MentionPubkeyCandidate[];
  repeatedMentions?: readonly RepeatedMention[];
}): string[] {
  const { text, selectedMentions, memberCandidates } = options;
  const repeatedMentions = options.repeatedMentions ?? [];
  const repeatedByLabel = new Map<string, string[]>();
  for (const repeated of repeatedMentions) {
    const label = normalizeDisplayName(repeated.displayName);
    repeatedByLabel.set(label, [
      ...(repeatedByLabel.get(label) ?? []),
      repeated.pubkey,
    ]);
  }
  const occurrenceCounts = new Map<string, number>();
  const candidates = mentionMatchCandidates(options);
  const winningPubkeys = new Set<string>();
  for (const { candidates: winners } of mentionOccurrences(text, candidates)) {
    const identities = new Set(
      winners.flatMap((match) =>
        match.pubkey ? [match.pubkey.toLowerCase()] : [],
      ),
    );
    if (identities.size > 1) {
      throw new Error(
        `The mention @${winners[0].displayName} is ambiguous. Choose a recipient from the mention picker.`,
      );
    }
    const label = normalizeDisplayName(winners[0].displayName);
    const repeated = repeatedByLabel.get(label);
    const primary = repeated
      ? boundPubkeyForLabel(label, selectedMentions)
      : undefined;
    if (
      repeated &&
      primary !== undefined &&
      identities.has(primary.toLowerCase())
    ) {
      const index = occurrenceCounts.get(label) ?? 0;
      occurrenceCounts.set(label, index + 1);
      if (index > 0) {
        const pubkey = repeated[index - 1];
        if (pubkey) winningPubkeys.add(pubkey);
        continue;
      }
    }
    for (const match of winners) {
      if (match.pubkey) winningPubkeys.add(match.pubkey);
    }
  }

  const pubkeys: string[] = [];
  for (const [, pubkey] of selectedMentions) {
    if (winningPubkeys.delete(pubkey)) pubkeys.push(pubkey);
  }
  for (const { pubkey } of repeatedMentions) {
    if (winningPubkeys.delete(pubkey)) pubkeys.push(pubkey);
  }
  for (const candidate of memberCandidates) {
    if (candidate.pubkey && winningPubkeys.delete(candidate.pubkey)) {
      pubkeys.push(candidate.pubkey);
    }
  }
  return pubkeys;
}
