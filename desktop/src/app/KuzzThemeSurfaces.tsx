import type { ReactNode } from "react";

export function GradientLayer() {
  return (
    <div
      aria-hidden="true"
      className="kuzz-theme-gradient-layer pointer-events-none absolute inset-0 -z-10"
      data-kuzz-gradient-layer
    >
      <div className="kuzz-theme-gradient-underlay absolute inset-0" />
      <div
        className="kuzz-theme-gradient-layer-light absolute inset-0 opacity-0"
        data-kuzz-gradient="light"
      />
      <div
        className="kuzz-theme-gradient-layer-dark absolute inset-0 opacity-0"
        data-kuzz-gradient="dark"
      />
    </div>
  );
}

export function ContentSurface({
  children,
  unframed = false,
  terminal,
}: {
  children: ReactNode;
  terminal?: ReactNode;
  /** Used by dedicated huddle windows, which should not resemble app cards. */
  unframed?: boolean;
}) {
  return (
    <div
      className={
        unframed
          ? "relative z-10 flex min-h-0 flex-1 flex-col overflow-hidden bg-background"
          : "relative z-10 mb-2 ml-px mr-2 mt-px flex min-h-0 flex-1 flex-col overflow-hidden rounded-2xl bg-background shadow-content-edge"
      }
      data-kuzz-content-surface
      data-kuzz-content-unframed={unframed ? true : undefined}
    >
      <div className="kuzz-content-primary flex min-h-0 flex-1 flex-col overflow-hidden">
        {children}
      </div>
      <div className="kuzz-terminal-dock-host" data-terminal-dock>
        {terminal}
      </div>
    </div>
  );
}
