import { t } from "@/shared/i18n";
import { toast } from "sonner";

import { invokeTauri } from "@/shared/api/tauri";

export function copyImageToClipboard(src: string | undefined) {
  if (!src) return;
  invokeTauri("copy_image_to_clipboard", { url: src })
    .then(() => toast.success(t("ui.clipboard.copied")))
    .catch((error: unknown) => {
      toast.error(
        error instanceof Error ? error.message : t("ui.image.copyFailed"),
      );
    });
}

export function downloadImage(src: string | undefined) {
  if (!src) return;
  invokeTauri("download_image", { url: src }).catch((error: unknown) => {
    toast.error(
      error instanceof Error ? error.message : t("ui.download.failed"),
    );
  });
}
