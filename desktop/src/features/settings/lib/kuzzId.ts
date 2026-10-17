/** Kuzz ID rules, matching the server: 3-32 of lowercase letters, digits, _. */
export const KUZZ_ID_RULES = "Use 3 to 32 lowercase letters, numbers or _";

const KUZZ_ID_PATTERN = /^[a-z0-9_]{3,32}$/;

/** A typed Kuzz ID as saved: trimmed, no leading "@", lowercased. */
export function normalizeKuzzId(input: string): string {
  return input.trim().replace(/^@/, "").toLowerCase();
}

/** Why `input` can't be saved, or null. Empty is fine: it clears the ID. */
export function kuzzIdError(input: string): string | null {
  const id = normalizeKuzzId(input);
  return id === "" || KUZZ_ID_PATTERN.test(id) ? null : KUZZ_ID_RULES;
}
