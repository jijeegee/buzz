import type { CSSProperties } from "react";

import { KUZZ_MARK_PATH, KUZZ_MARK_FRAMED_VIEWBOX } from "./kuzzLogoPaths";

export type KuzzLogoAnimationProps = {
  ariaLabel?: string;
  className?: string;
  fullScreen?: boolean;
  loop?: boolean;
  /** When looping, the mark pulses instead of morphing between plays. */
  loopRestSeconds?: number;
  reverse?: boolean;
  showBackground?: boolean;
  style?: CSSProperties;
  /** Kept for API compatibility; the Kuzz mark has no texture. */
  textured?: boolean;
  /** Kept for API compatibility; the Kuzz mark has one variant. */
  variant?: string;
};

/**
 * The Kuzz "K" mark in the container the upstream bee morph used, so every
 * loading and liveness surface keeps its layout and its own outer motion
 * (CSS pulse, staggered rows). A looping mark with a rest window pulses to
 * read as "alive".
 */
export default function KuzzLogoAnimation({
  ariaLabel = "Kuzz logo animation",
  className = "",
  fullScreen = true,
  loop = false,
  loopRestSeconds = 0,
  showBackground = true,
  style,
}: KuzzLogoAnimationProps) {
  const classes = [
    "kuzz-logo",
    fullScreen && "kuzz-logo--screen",
    !fullScreen && "kuzz-logo--compact",
    showBackground && "kuzz-logo--background",
    loop && loopRestSeconds > 0 && "kuzz-logo--pulse",
    className,
  ]
    .filter(Boolean)
    .join(" ");

  return (
    <div className={classes} style={style} role="img" aria-label={ariaLabel}>
      <svg
        className="kuzz-logo__mark"
        viewBox={KUZZ_MARK_FRAMED_VIEWBOX}
        aria-hidden="true"
        fill="currentColor"
      >
        <path d={KUZZ_MARK_PATH} />
      </svg>
    </div>
  );
}
