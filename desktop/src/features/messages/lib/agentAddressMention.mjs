import { normalizePubkey } from "../../../shared/lib/pubkey.ts";

export const AGENT_ADDRESS_MENTION_MARKER = "agent-address";
/** Marks a recipient Smart routing picked, not one the owner addressed. */
export const AUTO_ROUTE_MENTION_MARKER = "auto-route";

/**
 * Persist the subset of delivered mentions that came from the composer's
 * address tray. The ordinary `p` tag remains the notification mechanism;
 * this annotated reference is display metadata for reconstructing the tray
 * state when the message is rendered later. (Smart routing's picks are added
 * after the send, as `auto-route` tags on a delivery edit.)
 */
export function buildAgentAddressMentionTags(
  addressedPubkeys,
  deliveredPubkeys,
) {
  const delivered = new Set([...deliveredPubkeys].map(normalizePubkey));
  return [...new Set([...addressedPubkeys].map(normalizePubkey))]
    .filter((pubkey) => pubkey && delivered.has(pubkey))
    .map((pubkey) => ["mention", pubkey, AGENT_ADDRESS_MENTION_MARKER]);
}

function markedMentionPubkeys(tags, marker) {
  return [
    ...new Set(
      (tags ?? [])
        .filter(
          (tag) => tag[0] === "mention" && tag[2] === marker && Boolean(tag[1]),
        )
        .map((tag) => normalizePubkey(tag[1])),
    ),
  ];
}

/** Return the ordered, deduplicated agent-address recipients on an event. */
export function getAgentAddressMentionPubkeys(tags) {
  return markedMentionPubkeys(tags, AGENT_ADDRESS_MENTION_MARKER);
}

/** Return the ordered, deduplicated Smart routing recipients on an event. */
export function getAutoRouteMentionPubkeys(tags) {
  return markedMentionPubkeys(tags, AUTO_ROUTE_MENTION_MARKER);
}

/**
 * True for any send-time recipient annotation (`agent-address` or
 * `auto-route`): immutable metadata that edits carry from the original
 * rather than from the edited body.
 */
export function isAgentAddressMentionTag(tag) {
  return (
    tag[0] === "mention" &&
    (tag[2] === AGENT_ADDRESS_MENTION_MARKER ||
      tag[2] === AUTO_ROUTE_MENTION_MARKER)
  );
}
