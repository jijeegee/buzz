import { ThemeGrainientBackground } from "@/app/ThemeGrainientBackground";
import { FlappingBee } from "@/shared/ui/kuzz-logo/FlappingBee";
import { useT } from "@/shared/i18n";

/** Immediate feedback shown while the native huddle session is being prepared. */
export function HuddleStartingView() {
  const t = useT();
  return (
    <div
      aria-label={t("huddle.starting")}
      className="kuzz-setup-loading-shell flex min-h-0 flex-1 items-center justify-center overflow-hidden px-6 text-foreground"
      data-testid="huddle-starting-view"
      role="status"
    >
      <ThemeGrainientBackground />
      <span className="sr-only">{t("huddle.starting")}</span>
      <FlappingBee className="relative z-10 h-auto w-28" />
    </div>
  );
}
