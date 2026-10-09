/** Open layers that own Escape themselves (Radix dialogs, popovers, menus). */
const OPEN_LAYER_SELECTOR = [
  '[role="dialog"][data-state="open"]',
  '[role="alertdialog"][data-state="open"]',
  '[role="menu"][data-state="open"]',
  '[role="listbox"][data-state="open"]',
].join(", ");

type EscapeTarget = {
  tagName?: string;
  isContentEditable?: boolean;
  closest?: (selector: string) => unknown;
};

function isEditableTarget(target: unknown): boolean {
  if (!target || typeof target !== "object") return false;
  const element = target as EscapeTarget;
  const tag = element.tagName?.toUpperCase();
  if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return true;
  if (element.isContentEditable) return true;
  return Boolean(
    element.closest?.('[contenteditable=""], [contenteditable="true"]'),
  );
}

/**
 * Whether an Escape keydown should close Settings. Escape belongs to whatever
 * the user is in first: a text field (clearing or cancelling an edit), or an
 * open dialog, popover or menu. Only Escape from plain focus closes Settings.
 */
export function shouldEscapeCloseSettings(
  event: Pick<KeyboardEvent, "key" | "defaultPrevented" | "isComposing"> & {
    target: unknown;
  },
  root: Pick<Document, "querySelector"> | null = typeof document === "undefined"
    ? null
    : document,
): boolean {
  if (event.key !== "Escape" || event.defaultPrevented || event.isComposing) {
    return false;
  }
  if (isEditableTarget(event.target)) return false;
  return !root?.querySelector(OPEN_LAYER_SELECTOR);
}
