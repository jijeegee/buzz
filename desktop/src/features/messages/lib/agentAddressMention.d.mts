export const AGENT_ADDRESS_MENTION_MARKER: "agent-address";
export const AUTO_ROUTE_MENTION_MARKER: "auto-route";
export const SOFT_MENTION_MARKER: "soft";

export function buildSoftMentionTags(pubkeys: Iterable<string>): string[][];

export function getSoftMentionPubkeys(
  tags: readonly (readonly string[])[] | null | undefined,
): string[];

export function buildAgentAddressMentionTags(
  addressedPubkeys: Iterable<string>,
  deliveredPubkeys: Iterable<string>,
): string[][];

export function getAgentAddressMentionPubkeys(
  tags: readonly (readonly string[])[] | null | undefined,
): string[];

export function getAutoRouteMentionPubkeys(
  tags: readonly (readonly string[])[] | null | undefined,
): string[];

export function isAgentAddressMentionTag(tag: readonly string[]): boolean;
