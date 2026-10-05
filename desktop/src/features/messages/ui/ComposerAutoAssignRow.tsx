/**
 * Smart routing in the composer: only the quiet notice after a send that went
 * out unassigned. The pick itself is never previewed here; the sent message
 * shows who it was delivered to. Renders nothing when there is nothing to say.
 */
export function ComposerAutoAssignRow({ notice }: { notice: string | null }) {
  return notice ? (
    <div
      className="mb-2 text-xs text-muted-foreground"
      data-testid="composer-auto-assign-notice"
      role="status"
    >
      {notice}
    </div>
  ) : null;
}
