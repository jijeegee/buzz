/**
 * One-shot request for which Settings › Models tab opens next. A link from
 * elsewhere (the Agents page Channel routing card) sets it right before
 * navigating to Settings › Models; the panel reads it for its initial tab
 * and clears it after mount, so a later plain visit opens on the default
 * tab again. Read and clear are separate so a StrictMode double-invoked
 * state initializer sees the same answer both times.
 */
export type ModelsSettingsTab = "providers" | "defaults" | "tasks";

let requestedTab: ModelsSettingsTab | null = null;

const listeners = new Set<(tab: ModelsSettingsTab) => void>();

export function requestModelsSettingsTab(tab: ModelsSettingsTab): void {
  requestedTab = tab;
  // An already-open panel does not remount, so tell it directly.
  for (const listener of listeners) listener(tab);
}

export function subscribeModelsSettingsTabRequest(
  listener: (tab: ModelsSettingsTab) => void,
): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function readRequestedModelsSettingsTab(): ModelsSettingsTab | null {
  return requestedTab;
}

export function clearRequestedModelsSettingsTab(): void {
  requestedTab = null;
}
