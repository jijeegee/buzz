import { t } from "@/shared/i18n";
import { toast } from "sonner";

import { copyTextToSystemClipboard } from "@/shared/api/tauriMedia";

/**
 * Write text through the native clipboard integration.
 *
 * `html` is an optional richer flavor written in the same clipboard
 * transaction. External apps read `text`; Kuzz reads `html` on paste to
 * recover metadata the plain flavor deliberately omits (mention pubkeys).
 */
export async function writeTextToClipboard(
  text: string,
  html?: string,
): Promise<void> {
  await copyTextToSystemClipboard(text, html);
}

/** Copy text and show standard success/error feedback. */
export function copyTextToClipboard(
  text: string,
  successMessage?: string,
  html?: string,
) {
  void writeTextToClipboard(text, html)
    .then(() => {
      toast.success(successMessage ?? t("ui.clipboard.copied"));
    })
    .catch(() => {
      toast.error(t("ui.clipboard.copyFailed"));
    });
}
