import type { ReactNode } from "react";

import type {
  Channel,
  Identity,
  Profile,
  RelayEvent,
} from "@/shared/api/types";
import type { IdleAuxiliaryHeaderControls } from "./IdleAuxiliaryPanel";

/**
 * Owner view: the owner reads a chat of their agent that they are not in.
 * The agent's messages sit on the right and nothing can be sent.
 */
export type ChannelScreenOwnerView = {
  agentName: string;
  agentPubkey: string;
  /** Shown in place of the composer (ask the agent, remove it). */
  footer: (chatLabel: string) => ReactNode;
  subtitle: string;
};

export type ChannelScreenProps = {
  activeChannel: Channel | null;
  /**
   * When non-null, the main channel composer auto-submits once on mount after
   * loading the draft identified by this key. The route component clears the
   * `?autoSend` search param after the submit fires so back-navigation does
   * not re-trigger. Value must match the composer's `effectiveDraftKey`.
   */
  autoSendDraftKey: string | null;
  currentIdentity?: Identity;
  currentProfile?: Profile;
  idleAuxiliaryPanel?: ReactNode;
  idleAuxiliaryHeaderActions?: IdleAuxiliaryHeaderControls;
  idleAuxiliaryOverridesThread?: boolean;
  idleAuxiliaryTitle?: string;
  headerEndActions?: ReactNode;
  ownerView?: ChannelScreenOwnerView;
  onAddFiles?: () => void;
  onCloseIdleAuxiliaryPanel?: () => void;
  onCloseForumPost: () => void;
  onSelectForumPost: (postId: string) => void;
  selectedForumPostId: string | null;
  targetForumReplyId: string | null;
  targetMessageEvents: RelayEvent[];
  /** The route's target events are still being fetched. */
  isRouteTargetResolving?: boolean;
  targetMessageId: string | null;
  /** Exact clicked result id, retained after route target cleanup. */
  targetSearchMessageId?: string;
  /** Search text to highlight within the opened result message. */
  targetSearchQuery?: string;
};
