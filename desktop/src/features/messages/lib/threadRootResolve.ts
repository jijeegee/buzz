/**
 * Opening a thread from its root id: what one relay answer for the root
 * means (docs/message-sync.md#opening-a-thread-from-its-root-id). Shared
 * cases with mobile (`thread_root_provider.dart`):
 * test-fixtures/thread-open-root.json.
 */
import type { RelayEvent } from "@/shared/api/types";
import {
  KIND_DELETION,
  KIND_NIP29_DELETE_EVENT,
} from "@/shared/constants/kinds";

/** What is known of a thread's root so far. */
export type ThreadRootState = {
  /** The root as found: loaded, passed in, stored, then from the relay. */
  event: RelayEvent | null;
  /** Only on an explicit deletion (stored, live or in the relay's answer). */
  deleted: boolean;
};

/** What one answer says about the root; only `unresolved` is asked again. */
export type ThreadRootVerdict = "found" | "deleted" | "unresolved";

function inChannel(event: RelayEvent, channelId: string): boolean {
  return event.tags.some((tag) => tag[0] === "h" && tag[1] === channelId);
}

function targets(event: RelayEvent, rootId: string): boolean {
  return event.tags.some(
    (tag) => tag.length > 1 && tag[0] === "e" && tag[1] === rootId,
  );
}

/**
 * The verdict of `answer` (null when the request failed) for `rootId` in
 * `channelId`, and the root if it came back. An author's deletion (5) counts
 * only when signed by the root's author, so it needs the root in the same
 * answer or `knownRootAuthor` (the root already known from the store or the
 * loaded events). A removal (9005) always counts. A failed or empty answer
 * never means deleted.
 */
export function threadRootVerdict(
  answer: readonly RelayEvent[] | null,
  {
    channelId,
    rootId,
    knownRootAuthor = null,
  }: { channelId: string; rootId: string; knownRootAuthor?: string | null },
): { verdict: ThreadRootVerdict; root: RelayEvent | null } {
  if (answer === null) return { verdict: "unresolved", root: null };
  const root =
    answer.find(
      (event) => event.id === rootId && inChannel(event, channelId),
    ) ?? null;
  const author = root?.pubkey ?? knownRootAuthor;
  const deleted = answer.some(
    (event) =>
      inChannel(event, channelId) &&
      (event.kind === KIND_NIP29_DELETE_EVENT ||
        (event.kind === KIND_DELETION &&
          author !== null &&
          event.pubkey === author)) &&
      targets(event, rootId),
  );
  if (deleted) return { verdict: "deleted", root };
  if (root) return { verdict: "found", root };
  return { verdict: "unresolved", root: null };
}

/** A thread panel top row standing in for a root not found yet or deleted. */
export function isRootStandIn(
  message: { rootPlaceholder?: boolean; rootDeleted?: boolean } | null,
): boolean {
  return Boolean(message?.rootPlaceholder || message?.rootDeleted);
}
