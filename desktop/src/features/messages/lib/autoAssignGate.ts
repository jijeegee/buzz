import { isAgentMentionChannelType } from "@/features/agents/lib/agentAutocompleteEligibility";

/**
 * Whether Smart routing (channel routing mode `desktop-router`) may route
 * this composer's draft. Pure, so the full input space is table-tested; the
 * composer hook evaluates it once, on Enter.
 *
 * Only the user's own composer sends reach this path — agents post through
 * `buzz-cli`, never this UI — so AI-authored messages are never routed.
 */
export type AutoAssignGateInput = {
  /** The applied channel routing is Smart routing (not merely saved). */
  routerActive: boolean;
  /** The router model has an API key (Settings › Models › Task models). */
  routerReady: boolean;
  channelType: string | null | undefined;
  /** Editing an existing message never routes. */
  isEditing: boolean;
  /** Agents already addressed through the composer tray. */
  addressedAgentCount: number;
  /** Explicit `@mentions` (people or agents) in the draft. */
  explicitMentionCount: number;
  text: string;
  /** Agent members the router could pick from. */
  rosterSize: number;
};

export type AutoAssignSkipReason =
  | "router-off"
  | "router-not-ready"
  | "channel-type"
  | "editing"
  | "addressed"
  | "explicit-mention"
  | "too-short"
  | "no-roster";

export type AutoAssignDecision =
  | { run: true }
  | { run: false; reason: AutoAssignSkipReason };

/** Drafts shorter than this (whitespace removed) are never routed. */
export const MIN_ROUTABLE_CHARS = 2;

export function autoAssignDecision(
  input: AutoAssignGateInput,
): AutoAssignDecision {
  if (!input.routerActive) return { run: false, reason: "router-off" };
  if (!input.routerReady) return { run: false, reason: "router-not-ready" };
  // DMs already have exactly one audience; only stream/forum channels route.
  if (!isAgentMentionChannelType(input.channelType)) {
    return { run: false, reason: "channel-type" };
  }
  if (input.isEditing) return { run: false, reason: "editing" };
  if (input.addressedAgentCount > 0) return { run: false, reason: "addressed" };
  // An explicit @mention — of a person or an agent — is the user choosing;
  // the router never second-guesses it or intercepts a message for a human.
  if (input.explicitMentionCount > 0) {
    return { run: false, reason: "explicit-mention" };
  }
  if (input.text.replace(/\s/g, "").length < MIN_ROUTABLE_CHARS) {
    return { run: false, reason: "too-short" };
  }
  if (input.rosterSize === 0) return { run: false, reason: "no-roster" };
  return { run: true };
}
