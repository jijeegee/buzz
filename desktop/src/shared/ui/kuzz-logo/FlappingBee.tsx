import { KuzzMark } from "./KuzzMark";

/**
 * Loading-gate mark. Kuzz shows its static "K" here (the upstream flapping
 * bee is Kuzz branding); the export name is kept so callers stay unchanged.
 */
export function FlappingBee({ className }: { className?: string }) {
  return <KuzzMark className={className} />;
}
