import { KUZZ_WORDMARK_PATH, KUZZ_WORDMARK_VIEWBOX } from "./kuzzLogoPaths";

/** The "Kuzz" wordmark as a static SVG in `currentColor`. */
export function KuzzWordmark({
  className,
  title = "Kuzz",
}: {
  className?: string;
  title?: string;
}) {
  return (
    <svg
      aria-label={title}
      className={className}
      fill="currentColor"
      role="img"
      viewBox={KUZZ_WORDMARK_VIEWBOX}
    >
      <path d={KUZZ_WORDMARK_PATH} />
    </svg>
  );
}
