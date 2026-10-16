import { BuzzMark } from "./BuzzMark";

/**
 * Loading-gate mark. Kuzz shows its static "K" here (the upstream flapping
 * bee is Buzz branding); the export name is kept so callers stay unchanged.
 */
export function FlappingBee({ className }: { className?: string }) {
  return <BuzzMark className={className} />;
}
