import { AlertTriangle } from "lucide-react";

import {
  type RespondToAccess,
  respondToWideningWarning,
} from "../lib/respondToWidening";

/**
 * Top-of-dialog warning for an agent-authored draft whose respond-to access is
 * wider than the saved definition. The respond-to field itself sits in the
 * collapsed Advanced section, so a widening would otherwise be easy to miss.
 */
export function DraftAccessWideningBanner({
  savedAccess,
  draftAccess,
}: {
  /** Null when the draft creates a new agent (baseline: owner-only). */
  savedAccess: RespondToAccess | null;
  draftAccess: RespondToAccess;
}) {
  const warningText = respondToWideningWarning(savedAccess, draftAccess);
  if (!warningText) return null;
  return (
    <div
      className="flex items-start gap-2 rounded-xl border border-warning/30 bg-warning-bg px-3 py-2.5 lg:col-span-2"
      data-testid="draft-access-widening-warning"
      role="alert"
    >
      <AlertTriangle
        aria-hidden="true"
        className="mt-0.5 h-4 w-4 shrink-0 text-warning"
      />
      <p className="text-xs leading-5 text-warning">{warningText}</p>
    </div>
  );
}
