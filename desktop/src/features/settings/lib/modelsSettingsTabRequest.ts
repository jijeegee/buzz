/**
 * One-shot request for which Settings › Models tab opens next. A link from
 * elsewhere (the Agents page Channel routing card) sets it right before
 * navigating to Settings › Models; the panel reads it for its initial tab
 * and clears it after mount, so a later plain visit opens on the default
 * tab again. Read and clear are separate so a StrictMode double-invoked
 * state initializer sees the same answer both times.
 */
export type ModelsSettingsTab = "providers" | "tasks";

let requestedTab: ModelsSettingsTab | null = null;

export function requestModelsSettingsTab(tab: ModelsSettingsTab): void {
  requestedTab = tab;
}

export function readRequestedModelsSettingsTab(): ModelsSettingsTab | null {
  return requestedTab;
}

export function clearRequestedModelsSettingsTab(): void {
  requestedTab = null;
}
