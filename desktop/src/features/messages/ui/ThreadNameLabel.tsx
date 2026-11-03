import { useThreadNameLabel } from "../lib/useThreadName";

/**
 * Shared name in compact thread lists; absent names retain the existing
 * layout. Read-only: a row costs no live subscription (renames arrive on the
 * room bundle).
 */
export function ThreadNameLabel({
  channelId,
  threadId,
}: {
  channelId: string;
  threadId: string;
}) {
  const name = useThreadNameLabel(channelId, threadId);
  return name ? (
    <span
      className="mr-2 inline-block max-w-full truncate align-bottom font-medium"
      title={name}
    >
      {name}
    </span>
  ) : null;
}
