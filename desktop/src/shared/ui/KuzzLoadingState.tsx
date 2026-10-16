import { cn } from "@/shared/lib/cn";
import KuzzLogoAnimation from "@/shared/ui/kuzz-logo/KuzzLogoAnimation";

/** Centered, low-emphasis loading state for page and panel fetches. */
export function KuzzLoadingState({
  className,
  fill = false,
  label = "Loading",
}: {
  className?: string;
  fill?: boolean;
  label?: string;
}) {
  return (
    <div
      className={cn(
        "flex w-full items-center justify-center text-muted-foreground/45",
        fill ? "min-h-0 flex-1" : "min-h-[calc(100dvh-7rem)]",
        className,
      )}
      data-testid="kuzz-loading-state"
      role="status"
    >
      <KuzzLogoAnimation
        ariaLabel={label}
        className="kuzz-logo--scale-pulse"
        fullScreen={false}
        showBackground={false}
        style={{ width: "2rem" }}
        textured={false}
      />
    </div>
  );
}
