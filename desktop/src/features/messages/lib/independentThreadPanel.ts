import {
  formatTimelineMessages,
  isTimelineContentEvent,
} from "@/features/messages/lib/formatTimelineMessages";
import { buildThreadPanelData } from "@/features/messages/lib/threadPanel";
import {
  type ThreadRootState,
  threadRootVerdict,
} from "@/features/messages/lib/threadRootResolve";
import type { TimelineMessage } from "@/features/messages/types";
import type { RelayEvent } from "@/shared/api/types";
import {
  KIND_DELETION,
  KIND_NIP29_DELETE_EVENT,
  KIND_STREAM_MESSAGE,
} from "@/shared/constants/kinds";

/**
 * Aux events (edits/deletions/reactions) already loaded in the channel window
 * that reference `headId` via an `#e` tag. The thread head is the single
 * content event found by id, but its overlay events live alongside it in the
 * channel window — `formatTimelineMessages` only applies an edit/deletion when
 * the aux event sits in the SAME array as its target. Carrying them here keeps
 * the thread head byte-identical to the main timeline the instant the thread
 * opens, instead of rendering the un-edited original until the async
 * thread-aux backfill (`withThreadAux`) lands — the stale-edit-in-thread bug.
 *
 * Restricted to non-content kinds so reply content events (which also `#e` the
 * head as their parent) never leak in here — replies come from `replyEvents`.
 *
 * Deletion closure: an edit/reaction on the head can itself be deleted by a
 * kind:5/9005 that `#e`-references the *overlay's* id, not the head's. Those
 * deletions are copied too, so a deleted edit/reaction stays deleted in the
 * thread head instead of being resurrected until the async aux backfill lands
 * (or permanently if it fails) — mirroring the closure the aux-backfill paths
 * build (`mergeAuxEventsWithDeletionBackfill`).
 */
function headAuxEventsFromChannelWindow(
  channelEvents: RelayEvent[],
  headId: string,
): RelayEvent[] {
  const directAux = channelEvents.filter(
    (event) =>
      event.id !== headId &&
      !isTimelineContentEvent(event) &&
      event.tags.some((tag) => tag[0] === "e" && tag[1] === headId),
  );
  const directAuxIds = new Set(directAux.map((event) => event.id));
  const deletionsOfAux = channelEvents.filter(
    (event) =>
      (event.kind === KIND_DELETION ||
        event.kind === KIND_NIP29_DELETE_EVENT) &&
      !directAuxIds.has(event.id) &&
      event.tags.some((tag) => tag[0] === "e" && directAuxIds.has(tag[1])),
  );
  return [...directAux, ...deletionsOfAux];
}

export function buildIndependentThreadPanel(
  channelEvents: RelayEvent[],
  replyEvents: RelayEvent[],
  rootId: string | null,
  replyTargetId: string | null,
  expandedReplyIds: ReadonlySet<string>,
  ...formatArgs: Tail<Parameters<typeof formatTimelineMessages>>
) {
  return buildPanel(
    channelEvents,
    replyEvents,
    rootId,
    replyTargetId,
    expandedReplyIds,
    null,
    formatArgs,
  );
}

/**
 * How the channel screen's thread panel opens on the root id alone
 * (docs/message-sync.md#opening-a-thread-from-its-root-id).
 */
export type ThreadRootOpen = {
  channelId: string;
  /** What `useThreadRoot` found so far. */
  root: ThreadRootState;
  /** The thread's name shown in the top row until the root is found. */
  placeholderTitle: string;
};

/**
 * The thread panel of a thread opened by its root id: until the root is
 * found its top row is a placeholder with the thread name; on a deletion
 * proven by the loaded events or by `rootOpen.root`, the deleted row. The
 * replies draw either way. Only the channel screen opts in; other surfaces
 * keep `buildIndependentThreadPanel` (no head until the root is loaded).
 */
export function buildRootOpenedThreadPanel(
  rootOpen: ThreadRootOpen,
  channelEvents: RelayEvent[],
  replyEvents: RelayEvent[],
  rootId: string | null,
  replyTargetId: string | null,
  expandedReplyIds: ReadonlySet<string>,
  ...formatArgs: Tail<Parameters<typeof formatTimelineMessages>>
) {
  return buildPanel(
    channelEvents,
    replyEvents,
    rootId,
    replyTargetId,
    expandedReplyIds,
    rootOpen,
    formatArgs,
  );
}

function isDeletionOf(event: RelayEvent, id: string): boolean {
  return (
    (event.kind === KIND_DELETION || event.kind === KIND_NIP29_DELETE_EVENT) &&
    event.tags.some((tag) => tag[0] === "e" && tag[1] === id)
  );
}

/** The top row standing in for a root not found yet, or deleted. */
function rootStandIn(
  rootId: string,
  title: string,
  deleted: boolean,
): TimelineMessage {
  return {
    id: rootId,
    rootId: null,
    parentId: null,
    depth: 0,
    createdAt: 0,
    time: "",
    body: "",
    tags: [],
    author: title,
    ...(deleted ? { rootDeleted: true } : { rootPlaceholder: true }),
  };
}

/** A real head for a found root that formats to no row: no body. */
function headlessHead(
  head: RelayEvent,
  rootId: string,
  formatArgs: Tail<Parameters<typeof formatTimelineMessages>>,
): TimelineMessage | null {
  const row = formatTimelineMessages(
    [{ ...head, kind: KIND_STREAM_MESSAGE, content: "" }],
    ...formatArgs,
  ).find((message) => message.id === rootId);
  return row
    ? { ...row, kind: head.kind, rootId: null, parentId: null, depth: 0 }
    : null;
}

function buildPanel(
  channelEvents: RelayEvent[],
  replyEvents: RelayEvent[],
  rootId: string | null,
  replyTargetId: string | null,
  expandedReplyIds: ReadonlySet<string>,
  rootOpen: ThreadRootOpen | null,
  formatArgs: Tail<Parameters<typeof formatTimelineMessages>>,
) {
  if (!rootId) {
    return {
      ...buildThreadPanelData([], null, replyTargetId, expandedReplyIds),
      messages: [],
    };
  }
  const head =
    channelEvents.find((event) => event.id === rootId) ??
    (rootOpen?.root.event?.id === rootId ? rootOpen.root.event : undefined);
  // Dedup the channel-window head aux against `replyEvents`: `withThreadAux`
  // fetches the same overlays by reference, so both sources can carry an edit.
  const replyEventIds = new Set(replyEvents.map((event) => event.id));
  let headAux = head
    ? headAuxEventsFromChannelWindow(channelEvents, rootId).filter(
        (event) => !replyEventIds.has(event.id),
      )
    : [];
  let replies = replyEvents;
  let deleted = false;
  if (rootOpen) {
    // A deletion in the loaded events counts only as the fixture says: a
    // removal (9005), or a 5 signed by the root's author. Any other deletion
    // of the root is dropped so it cannot hide a found root.
    const deletions = [...channelEvents, ...replyEvents].filter((event) =>
      isDeletionOf(event, rootId),
    );
    deleted =
      rootOpen.root.deleted ||
      threadRootVerdict(head ? [head, ...deletions] : deletions, {
        channelId: rootOpen.channelId,
        rootId,
        knownRootAuthor: head?.pubkey ?? null,
      }).verdict === "deleted";
    if (!deleted && deletions.length > 0) {
      headAux = headAux.filter((event) => !isDeletionOf(event, rootId));
      replies = replies.filter((event) => !isDeletionOf(event, rootId));
    }
  }
  const events = head ? [head, ...headAux, ...replies] : replies;
  let messages = formatTimelineMessages(events, ...formatArgs);
  if (rootOpen && (deleted || !messages.some((m) => m.id === rootId))) {
    // A found root of a kind the timeline does not draw still becomes a real
    // head (author and time, no body) instead of a placeholder forever.
    const headless =
      head && !deleted ? headlessHead(head, rootId, formatArgs) : null;
    messages = [
      headless ?? rootStandIn(rootId, rootOpen.placeholderTitle, deleted),
      ...messages.filter((message) => message.id !== rootId),
    ];
  }
  return {
    ...buildThreadPanelData(messages, rootId, replyTargetId, expandedReplyIds),
    messages,
  };
}

type Tail<T extends readonly unknown[]> = T extends readonly [
  unknown,
  ...infer R,
]
  ? R
  : never;
