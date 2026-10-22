import { EyeOff } from "lucide-react";

import { useT } from "@/shared/i18n";

/**
 * Stands in for a message whose author I blocked (shared channels still
 * deliver them). One line; clicking it shows the message.
 */
export function BlockedMessageNotice({ onShow }: { onShow: () => void }) {
  const t = useT();
  return (
    <button
      aria-label={t("messages.blocked.showLabel")}
      className="mx-1 flex w-fit items-center gap-2 rounded-md px-2 py-1 text-left text-xs text-muted-foreground transition-colors hover:bg-muted/60 hover:text-foreground focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring"
      data-testid="blocked-message-notice"
      onClick={onShow}
      type="button"
    >
      <EyeOff aria-hidden className="h-3.5 w-3.5 shrink-0" />
      <span>{t("messages.blocked.label")}</span>
      <span className="font-medium underline-offset-2 hover:underline">
        {t("messages.blocked.show")}
      </span>
    </button>
  );
}
