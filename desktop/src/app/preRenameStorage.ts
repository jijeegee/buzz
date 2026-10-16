/**
 * One-time copy of browser storage written before the product rename.
 *
 * The desktop shell copies the old WebView profile into the new app's data
 * folder, so this window starts with keys under the former product name.
 * Copy each to its current name (never overwriting a current key) and map the
 * few stored values that name a renamed theme or runtime. Old keys are left in
 * place so an older build still finds them. Imported first by `main.tsx`,
 * before any module reads storage.
 */
const DONE_KEY = "kuzz:pre-rename-storage-imported.v1";

const LEGACY_WORD = /buzz|Buzz|BUZZ/g; // kuzz-rename: keep
const CURRENT_WORD: Record<string, string> = {
  buzz: "kuzz", // kuzz-rename: keep
  Buzz: "Kuzz", // kuzz-rename: keep
  BUZZ: "KUZZ", // kuzz-rename: keep
};

/** Stored values that name something renamed (theme ids, bundled runtime). */
const LEGACY_VALUES: Record<string, string> = {
  buzz: "kuzz", // kuzz-rename: keep
  "buzz-dark": "kuzz-dark", // kuzz-rename: keep
  "buzz-agent": "kuzz-agent", // kuzz-rename: keep
  "buzz-acp": "kuzz-acp", // kuzz-rename: keep
};

export function currentStorageKey(key: string): string {
  return key.replace(LEGACY_WORD, (word) => CURRENT_WORD[word] ?? word);
}

export function currentStorageValue(value: string): string {
  const exact = LEGACY_VALUES[value];
  if (exact !== undefined) return exact;
  let next = value;
  for (const [legacy, current] of Object.entries(LEGACY_VALUES)) {
    next = next.split(`"${legacy}"`).join(`"${current}"`);
  }
  return next;
}

export function importPreRenameStorage(storage: Storage): number {
  if (storage.getItem(DONE_KEY) !== null) return 0;
  const keys: string[] = [];
  for (let index = 0; index < storage.length; index += 1) {
    const key = storage.key(index);
    if (key !== null) keys.push(key);
  }
  let copied = 0;
  for (const key of keys) {
    const next = currentStorageKey(key);
    if (next === key || storage.getItem(next) !== null) continue;
    const value = storage.getItem(key);
    if (value === null) continue;
    storage.setItem(next, currentStorageValue(value));
    copied += 1;
  }
  storage.setItem(DONE_KEY, String(Date.now()));
  return copied;
}

try {
  if (typeof window !== "undefined" && window.localStorage) {
    importPreRenameStorage(window.localStorage);
  }
} catch (error) {
  console.warn("pre-rename storage import failed", error);
}
