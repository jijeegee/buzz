import * as React from "react";
import type {
  AgentPersona,
  Channel,
  ManagedAgent,
  Profile,
  RelayAgent,
} from "@/shared/api/types";
import {
  normalizePubkey,
  truncateNpub,
  UNKNOWN_USER_LABEL,
} from "@/shared/lib/pubkey";

export { truncateNpub, UNKNOWN_USER_LABEL };

export type ProfileChannelLink = {
  id: string;
  name: string;
};

export type ProfilePanelView =
  | "summary"
  | "instructions"
  | "info"
  | "configuration"
  | "diagnostics"
  | "memories"
  | "channels"
  | "logs";

export type ProfilePanelTab = "info" | "runtime" | "channels" | "memories";

export const PROFILE_PANEL_VIEW_TITLES: Record<ProfilePanelView, string> = {
  summary: "Profile",
  instructions: "Agent instructions",
  info: "Agent info",
  configuration: "Runtime",
  diagnostics: "Harness log",
  memories: "Memories",
  channels: "Groups",
  logs: "Harness log",
};

const PROFILE_PANEL_VIEWS = new Set<ProfilePanelView>(
  Object.keys(PROFILE_PANEL_VIEW_TITLES) as ProfilePanelView[],
);

const PROFILE_PANEL_TABS = new Set<ProfilePanelTab>([
  "info",
  "runtime",
  "channels",
  "memories",
]);

const LEGACY_PROFILE_PANEL_VIEW_ALIASES: Record<string, ProfilePanelView> = {
  model: "configuration",
  settings: "configuration",
};

export function parseProfilePanelView(value: unknown): ProfilePanelView | null {
  if (typeof value !== "string") {
    return null;
  }

  if (PROFILE_PANEL_VIEWS.has(value as ProfilePanelView)) {
    return value as ProfilePanelView;
  }

  return LEGACY_PROFILE_PANEL_VIEW_ALIASES[value] ?? null;
}

export function profilePanelViewFromSearch(value: unknown): ProfilePanelView {
  return parseProfilePanelView(value) ?? "summary";
}

export function parseProfilePanelTab(value: unknown): ProfilePanelTab | null {
  if (typeof value !== "string") {
    return null;
  }

  if (PROFILE_PANEL_TABS.has(value as ProfilePanelTab)) {
    return value as ProfilePanelTab;
  }

  return null;
}

export function profilePanelTabFromSearch(value: unknown): ProfilePanelTab {
  return parseProfilePanelTab(value) ?? "info";
}

export function profilePanelTargetKey(
  pubkey: string | undefined,
  personaId: string | undefined,
): string {
  return pubkey ?? `persona:${personaId ?? "unknown"}`;
}

export type UserProfilePanelProps = {
  canResetWidth?: boolean;
  currentPubkey?: string;
  isSinglePanelView?: boolean;
  layout?: "standalone" | "split";
  onClose: () => void;
  onOpenDm?: (pubkeys: string[]) => Promise<void> | void;
  onOpenProfile?: (pubkey: string) => void;
  onResetWidth?: () => void;
  onResizeStart?: (event: React.PointerEvent<HTMLButtonElement>) => void;
  onTabChange?: (tab: ProfilePanelTab, options?: { replace?: boolean }) => void;
  onViewChange?: (
    view: ProfilePanelView,
    options?: { replace?: boolean },
  ) => void;
  persona?: AgentPersona;
  pubkey?: string;
  splitPaneClamp?: boolean;
  tab?: ProfilePanelTab;
  view?: ProfilePanelView;
  widthPx: number;
  transparentChrome?: boolean;
};

export function deriveProfileChannels(
  pubkeyLower: string,
  relayAgent: RelayAgent | undefined,
  managedAgent: ManagedAgent | undefined,
  channels: Channel[] | undefined,
): ProfileChannelLink[] {
  const links = new Map<string, ProfileChannelLink>();
  const channelsByName = new Map(
    channels?.map((channel) => [channel.name, channel]) ?? [],
  );

  relayAgent?.channels.forEach((name, index) => {
    const channel = channelsByName.get(name);
    const id = relayAgent.channelIds[index] ?? channel?.id ?? name;
    links.set(id, { id, name });
  });

  if (managedAgent && channels) {
    for (const channel of channels) {
      const isMember = channel.memberPubkeys.some(
        (memberPubkey) => memberPubkey.toLowerCase() === pubkeyLower,
      );
      if (isMember) {
        links.set(channel.id, { id: channel.id, name: channel.name });
      }
    }
  }

  return [...links.values()].sort((left, right) =>
    left.name.localeCompare(right.name),
  );
}

export function getRelayAgentChannelIds(
  relayAgents: readonly RelayAgent[] | undefined,
  agentPubkey: string,
): string[] {
  const normalized = normalizePubkey(agentPubkey);
  const agent = (relayAgents ?? []).find(
    (candidate) => normalizePubkey(candidate.pubkey) === normalized,
  );
  return agent?.channelIds ?? [];
}

export function buildPersonaDraftProfile(persona: AgentPersona): Profile {
  return {
    pubkey: "",
    displayName: persona.displayName,
    avatarUrl: persona.avatarUrl,
    about: null,
    nip05Handle: null,
    ownerPubkey: null,
    // Draft profile synthesised from persona config — not backed by a kind:0 event.
    hasProfileEvent: false,
  };
}

export function resolvePanelProfile({
  managedAgent,
  persona,
  profile,
}: {
  managedAgent: ManagedAgent | undefined;
  persona: AgentPersona | undefined;
  profile: Profile | undefined;
}): Profile | undefined {
  const draft = persona ? buildPersonaDraftProfile(persona) : undefined;
  // An agent falls back to its own name and avatar, never its preset's.
  const baseProfile =
    profile ??
    (draft && managedAgent
      ? {
          ...draft,
          displayName: managedAgent.name,
          avatarUrl: managedAgent.avatarUrl,
        }
      : draft);
  return withProfileAvatarFallback(baseProfile, [
    managedAgent ? managedAgent.avatarUrl : persona?.avatarUrl,
  ]);
}

export function resolveProfileAvatarUrl(
  ...candidates: Array<string | null | undefined>
): string | null {
  for (const candidate of candidates) {
    const trimmed = candidate?.trim();
    if (trimmed) return trimmed;
  }
  return null;
}

export function withProfileAvatarFallback(
  profile: Profile | undefined,
  fallbackAvatarUrls: Array<string | null | undefined>,
): Profile | undefined {
  const profileAvatarUrl = normalizeProfileFallbackAvatarUrl(
    profile?.avatarUrl,
  );
  const avatarUrl = resolveProfileAvatarUrl(
    profileAvatarUrl,
    ...fallbackAvatarUrls.map((avatarUrl) =>
      normalizeProfileFallbackAvatarUrl(avatarUrl),
    ),
  );
  return profile && avatarUrl !== profile.avatarUrl
    ? { ...profile, avatarUrl }
    : profile;
}

function normalizeProfileFallbackAvatarUrl(
  avatarUrl: string | null | undefined,
): string | null {
  const trimmed = avatarUrl?.trim();
  if (!trimmed) return null;
  return trimmed;
}

export function resolveProfileDisplayName({
  persona,
  profile,
  pubkey,
}: {
  persona: AgentPersona | undefined;
  profile: Profile | undefined;
  pubkey: string | null;
}) {
  return (
    profile?.displayName ??
    persona?.displayName ??
    (pubkey ? UNKNOWN_USER_LABEL : "Agent")
  );
}

export function resolveOwnerHandle(
  profile: Profile | undefined,
  currentPubkey: string | undefined,
) {
  if (currentPubkey === undefined) {
    return null;
  }

  return (
    profile?.nip05Handle?.trim() ||
    profile?.displayName?.trim() ||
    UNKNOWN_USER_LABEL
  );
}

/** An agent shows its own instructions; a preset profile shows the preset's. */
export function resolveAgentInstruction(
  managedAgent: ManagedAgent | undefined,
  persona: AgentPersona | undefined,
) {
  if (managedAgent) return managedAgent.systemPrompt?.trim() || null;
  return persona?.systemPrompt.trim() || null;
}

export function useRetainedPersona(
  sourcePersona: AgentPersona | undefined,
  profileIdentityKey: string,
) {
  const [retainedPersona, setRetainedPersona] = React.useState<{
    key: string;
    persona: AgentPersona;
  } | null>(null);

  React.useEffect(() => {
    if (!sourcePersona) return;
    setRetainedPersona({ key: profileIdentityKey, persona: sourcePersona });
  }, [profileIdentityKey, sourcePersona]);

  return (
    sourcePersona ??
    (retainedPersona?.key === profileIdentityKey
      ? retainedPersona.persona
      : undefined)
  );
}
