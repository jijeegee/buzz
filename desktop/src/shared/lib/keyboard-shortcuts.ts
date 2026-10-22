import { t } from "@/shared/i18n";
import { isMacPlatform } from "@/shared/lib/platform";

export const HUDDLE_SHORTCUT_EVENT = "kuzz:huddle-shortcut";

export type HuddleShortcutDetail = {
  channelId: string;
};

export type ShortcutCategory =
  | "Navigation"
  | "Messages"
  | "Formatting"
  | "Zoom";

export type KeyboardShortcut = {
  id: string;
  label: string;
  description: string;
  keys: string;
  keysWindows: string;
  category: ShortcutCategory;
};

export const KEYBOARD_SHORTCUTS: KeyboardShortcut[] = [
  // Navigation
  {
    id: "quick-search",
    get label() {
      return t("ui.shortcuts.quickSearch.label");
    },
    get description() {
      return t("ui.shortcuts.quickSearch.description");
    },
    keys: "⌘K",
    keysWindows: "Ctrl+K",
    category: "Navigation",
  },
  {
    id: "browse-channels",
    get label() {
      return t("ui.shortcuts.browseChannels.label");
    },
    get description() {
      return t("ui.shortcuts.browseChannels.description");
    },
    keys: "⇧⌘O",
    keysWindows: "Shift+Ctrl+O",
    category: "Navigation",
  },
  {
    id: "browse-dms",
    get label() {
      return t("ui.shortcuts.browseDms.label");
    },
    get description() {
      return t("ui.shortcuts.browseDms.description");
    },
    keys: "⇧⌘K",
    keysWindows: "Shift+Ctrl+K",
    category: "Navigation",
  },
  {
    id: "new-channel",
    get label() {
      return t("ui.shortcuts.newChannel.label");
    },
    get description() {
      return t("ui.shortcuts.newChannel.description");
    },
    keys: "⇧⌘N",
    keysWindows: "Shift+Ctrl+N",
    category: "Navigation",
  },
  {
    id: "open-settings",
    get label() {
      return t("ui.shortcuts.openSettings.label");
    },
    get description() {
      return t("ui.shortcuts.openSettings.description");
    },
    keys: "⌘,",
    keysWindows: "Ctrl+,",
    category: "Navigation",
  },
  {
    id: "go-back",
    get label() {
      return t("ui.shortcuts.goBack.label");
    },
    get description() {
      return t("ui.shortcuts.goBack.description");
    },
    keys: "⌘[",
    keysWindows: "Alt+←",
    category: "Navigation",
  },
  {
    id: "go-forward",
    get label() {
      return t("ui.shortcuts.goForward.label");
    },
    get description() {
      return t("ui.shortcuts.goForward.description");
    },
    keys: "⌘]",
    keysWindows: "Alt+→",
    category: "Navigation",
  },
  {
    id: "find-in-channel",
    get label() {
      return t("ui.shortcuts.findInChannel.label");
    },
    get description() {
      return t("ui.shortcuts.findInChannel.description");
    },
    keys: "⌘F",
    keysWindows: "Ctrl+F",
    category: "Navigation",
  },
  {
    id: "go-home",
    get label() {
      return t("ui.shortcuts.goHome.label");
    },
    get description() {
      return t("ui.shortcuts.goHome.description");
    },
    keys: "⇧⌘A",
    keysWindows: "Shift+Ctrl+A",
    category: "Navigation",
  },
  {
    id: "toggle-sidebar",
    get label() {
      return t("ui.shortcuts.toggleSidebar.label");
    },
    get description() {
      return t("ui.shortcuts.toggleSidebar.description");
    },
    keys: "⌘S",
    keysWindows: "Ctrl+S",
    category: "Navigation",
  },
  {
    id: "mark-current-read",
    get label() {
      return t("ui.shortcuts.markCurrentRead.label");
    },
    get description() {
      return t("ui.shortcuts.markCurrentRead.description");
    },
    keys: "Escape",
    keysWindows: "Escape",
    category: "Navigation",
  },
  {
    id: "mark-all-read",
    get label() {
      return t("ui.shortcuts.markAllRead.label");
    },
    get description() {
      return t("ui.shortcuts.markAllRead.description");
    },
    keys: "⇧Escape",
    keysWindows: "Shift+Escape",
    category: "Navigation",
  },

  // Zoom
  {
    id: "zoom-in",
    get label() {
      return t("ui.shortcuts.zoomIn.label");
    },
    get description() {
      return t("ui.shortcuts.zoomIn.description");
    },
    keys: "⌘+",
    keysWindows: "Ctrl+=",
    category: "Zoom",
  },
  {
    id: "zoom-out",
    get label() {
      return t("ui.shortcuts.zoomOut.label");
    },
    get description() {
      return t("ui.shortcuts.zoomOut.description");
    },
    keys: "⌘-",
    keysWindows: "Ctrl+-",
    category: "Zoom",
  },
  {
    id: "zoom-reset",
    get label() {
      return t("ui.shortcuts.zoomReset.label");
    },
    get description() {
      return t("ui.shortcuts.zoomReset.description");
    },
    keys: "⌘0",
    keysWindows: "Ctrl+0",
    category: "Zoom",
  },

  // Messages
  {
    id: "send-message",
    get label() {
      return t("ui.shortcuts.sendMessage.label");
    },
    get description() {
      return t("ui.shortcuts.sendMessage.description");
    },
    keys: "Enter",
    keysWindows: "Enter",
    category: "Messages",
  },
  {
    id: "new-line",
    get label() {
      return t("ui.shortcuts.newLine.label");
    },
    get description() {
      return t("ui.shortcuts.newLine.description");
    },
    keys: "Shift+Enter",
    keysWindows: "Shift+Enter",
    category: "Messages",
  },
  {
    id: "always-address-agent",
    get label() {
      return t("ui.shortcuts.alwaysAddressAgent.label");
    },
    get description() {
      return t("ui.shortcuts.alwaysAddressAgent.description");
    },
    keys: "⇧⌘M",
    keysWindows: "Ctrl+Shift+M",
    category: "Messages",
  },
  {
    id: "publish-note",
    get label() {
      return t("ui.shortcuts.publishNote.label");
    },
    get description() {
      return t("ui.shortcuts.publishNote.description");
    },
    keys: "⌘Enter",
    keysWindows: "Ctrl+Enter",
    category: "Messages",
  },
  {
    id: "close-dialog",
    get label() {
      return t("ui.shortcuts.closeDialog.label");
    },
    get description() {
      return t("ui.shortcuts.closeDialog.description");
    },
    keys: "Escape",
    keysWindows: "Escape",
    category: "Messages",
  },
  {
    id: "toggle-huddle",
    get label() {
      return t("ui.shortcuts.toggleHuddle.label");
    },
    get description() {
      return t("ui.shortcuts.toggleHuddle.description");
    },
    keys: "Ctrl+Shift+Space",
    keysWindows: "Ctrl+Shift+Space",
    category: "Messages",
  },
  {
    id: "push-to-talk",
    get label() {
      return t("ui.shortcuts.pushToTalk.label");
    },
    get description() {
      return t("ui.shortcuts.pushToTalk.description");
    },
    keys: "Ctrl+Space",
    keysWindows: "Ctrl+Space",
    category: "Messages",
  },

  // Formatting
  {
    id: "format-bold",
    get label() {
      return t("ui.shortcuts.formatBold.label");
    },
    get description() {
      return t("ui.shortcuts.formatBold.description");
    },
    keys: "⌘B",
    keysWindows: "Ctrl+B",
    category: "Formatting",
  },
  {
    id: "format-italic",
    get label() {
      return t("ui.shortcuts.formatItalic.label");
    },
    get description() {
      return t("ui.shortcuts.formatItalic.description");
    },
    keys: "⌘I",
    keysWindows: "Ctrl+I",
    category: "Formatting",
  },
  {
    id: "format-strikethrough",
    get label() {
      return t("ui.shortcuts.formatStrikethrough.label");
    },
    get description() {
      return t("ui.shortcuts.formatStrikethrough.description");
    },
    keys: "⌘⇧X",
    keysWindows: "Ctrl+Shift+X",
    category: "Formatting",
  },
  {
    id: "format-code",
    get label() {
      return t("ui.shortcuts.formatCode.label");
    },
    get description() {
      return t("ui.shortcuts.formatCode.description");
    },
    keys: "⌘E",
    keysWindows: "Ctrl+E",
    category: "Formatting",
  },
  {
    id: "format-link",
    get label() {
      return t("ui.shortcuts.formatLink.label");
    },
    get description() {
      return t("ui.shortcuts.formatLink.description");
    },
    keys: "⌘K",
    keysWindows: "Ctrl+K",
    category: "Formatting",
  },
];

const CATEGORY_ORDER: ShortcutCategory[] = [
  "Navigation",
  "Messages",
  "Formatting",
  "Zoom",
];

/** Translated group title for a shortcut category (the category values are ids). */
export function shortcutCategoryLabel(category: ShortcutCategory): string {
  switch (category) {
    case "Navigation":
      return t("ui.shortcuts.category.navigation");
    case "Messages":
      return t("ui.shortcuts.category.messages");
    case "Formatting":
      return t("ui.shortcuts.category.formatting");
    case "Zoom":
      return t("ui.shortcuts.category.zoom");
  }
}

export function getShortcutsByCategory(): Map<
  ShortcutCategory,
  KeyboardShortcut[]
> {
  const map = new Map<ShortcutCategory, KeyboardShortcut[]>();
  for (const cat of CATEGORY_ORDER) {
    map.set(
      cat,
      KEYBOARD_SHORTCUTS.filter((s) => s.category === cat),
    );
  }
  return map;
}

export function getPlatformKeys(shortcut: KeyboardShortcut): string {
  return isMacPlatform() ? shortcut.keys : shortcut.keysWindows;
}

/**
 * Platform-appropriate key hint for a shortcut in {@link KEYBOARD_SHORTCUTS},
 * or null when the id is unknown. Use this for inline hints (menus, tooltips)
 * so they stay in sync with the canonical shortcut registry.
 */
export function getPlatformKeysById(id: string): string | null {
  const shortcut = KEYBOARD_SHORTCUTS.find((s) => s.id === id);
  return shortcut ? getPlatformKeys(shortcut) : null;
}
