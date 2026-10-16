import { KUZZ_MARK_PATH, KUZZ_MARK_FRAMED_VIEWBOX } from "./kuzzLogoPaths";

/**
 * The Kuzz "K" mark as a plain static SVG in `currentColor`, so it tints per
 * theme and paints complete on the very first frame.
 */
export function BuzzMark({ className }: { className?: string }) {
  return (
    <svg
      aria-hidden="true"
      className={["buzz-mark", className].filter(Boolean).join(" ")}
      viewBox={KUZZ_MARK_FRAMED_VIEWBOX}
      fill="currentColor"
    >
      <path d={KUZZ_MARK_PATH} />
    </svg>
  );
}
