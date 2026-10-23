import type { AcpRuntimeCatalogEntry } from "@/shared/api/types";
import { t } from "@/shared/i18n";

/**
 * Availability warning sentence for the agent-definition dialog.
 * Returns null when the runtime is available (no warning to show).
 *
 * Non-empty install hints are appended so a not-installed or
 * adapter-missing runtime tells the user the actual next step (presets
 * always carry one; custom harnesses may leave it empty — guard on the
 * trimmed value to avoid a dangling "X is not installed. ").
 */
export function runtimeAvailabilityWarning(
  runtime: AcpRuntimeCatalogEntry,
): string | null {
  if (runtime.availability === "available") {
    return null;
  }
  const hint = runtime.installHint.trim();
  const withHint = (base: string) => (hint ? `${base} ${hint}` : base);
  switch (runtime.availability) {
    case "adapter_missing":
      return withHint(
        t("agents.runtimeWarning.adapterMissing", { runtime: runtime.label }),
      );
    case "adapter_outdated":
      return t("agents.runtimeWarning.adapterOutdated", {
        runtime: runtime.label,
      });
    default:
      return runtime.requiresExternalCli
        ? withHint(
            t("agents.runtimeWarning.cliMissing", { runtime: runtime.label }),
          )
        : withHint(
            t("agents.runtimeWarning.notInstalled", { runtime: runtime.label }),
          );
  }
}
