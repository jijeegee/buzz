import { invokeTauri } from "@/shared/api/tauri";
import { getStorageItem } from "@/shared/lib/safeStorage";
import { readmitRelay } from "@/features/agents/managedAgentRelayCleanup";
import { migrateLegacyCommunityStorage } from "./communityStorage";

const KUZZ_COMMUNITIES_KEY = "kuzz-communities";
const KUZZ_ACTIVE_COMMUNITY_KEY = "kuzz-active-community-id";
const KUZZ_ONBOARDING_COMPLETION_STORAGE_KEY_PREFIX =
  "kuzz-onboarding-complete.v1:";
const LOCAL_DEV_RELAY_URLS = new Set([
  "ws://localhost:3000",
  "ws://127.0.0.1:3000",
]);

type LegacyCommunityStorageSnapshot = {
  workspaces: string | null;
  activeWorkspaceId: string | null;
  onboardingCompletions: Array<{
    pubkey: string;
    value: string;
  }>;
};

type StoredCommunity = {
  relayUrl?: unknown;
};

function parseCommunityList(raw: string | null): StoredCommunity[] | null {
  if (!raw) {
    return null;
  }

  try {
    const parsed: unknown = JSON.parse(raw);
    return Array.isArray(parsed) ? (parsed as StoredCommunity[]) : null;
  } catch {
    return null;
  }
}

function normalizeRelayUrl(relayUrl: string) {
  return relayUrl.trim().replace(/\/$/, "");
}

function hasOnlyLocalDevCommunity(raw: string | null): boolean {
  const communities = parseCommunityList(raw);
  return (
    communities?.length === 1 &&
    typeof communities[0]?.relayUrl === "string" &&
    LOCAL_DEV_RELAY_URLS.has(normalizeRelayUrl(communities[0].relayUrl))
  );
}

function hasNonLocalCurrentCommunities(raw: string | null): boolean {
  const communities = parseCommunityList(raw);
  return (
    communities !== null &&
    communities.length > 0 &&
    !hasOnlyLocalDevCommunity(raw)
  );
}

function shouldWriteLegacyCommunities({
  currentCommunitiesRaw,
  legacyCommunitiesRaw,
}: {
  currentCommunitiesRaw: string | null;
  legacyCommunitiesRaw: string | null;
}) {
  const legacyCommunities = parseCommunityList(legacyCommunitiesRaw);
  if (!legacyCommunities || legacyCommunities.length === 0) {
    return false;
  }

  return !hasNonLocalCurrentCommunities(currentCommunitiesRaw);
}

/** Relay URLs in `nextRaw` that aren't already saved in `currentRaw`. */
function newRelayUrls(currentRaw: string | null, nextRaw: string): string[] {
  // Deliberately raw: native canonicalizes and validates on re-add, and a
  // JavaScript notion of "same relay" can skip a URL native would admit.
  const seen = new Set(
    (parseCommunityList(currentRaw) ?? []).map((c) => c.relayUrl),
  );
  const introduced: string[] = [];
  for (const { relayUrl } of parseCommunityList(nextRaw) ?? []) {
    if (typeof relayUrl !== "string" || seen.has(relayUrl)) continue;
    seen.add(relayUrl);
    introduced.push(relayUrl);
  }
  return introduced;
}

export function applyLegacyCommunityStorage(
  legacyStorage: LegacyCommunityStorageSnapshot,
  storage: Storage = window.localStorage,
): string[] {
  const currentCommunitiesRaw = storage.getItem(KUZZ_COMMUNITIES_KEY);
  const shouldWriteCommunities = shouldWriteLegacyCommunities({
    currentCommunitiesRaw,
    legacyCommunitiesRaw: legacyStorage.workspaces,
  });

  let introducedRelayUrls: string[] = [];
  if (shouldWriteCommunities && legacyStorage.workspaces) {
    storage.setItem(KUZZ_COMMUNITIES_KEY, legacyStorage.workspaces);
    introducedRelayUrls = newRelayUrls(
      currentCommunitiesRaw,
      legacyStorage.workspaces,
    );
  }

  // The list is saved, so its new relays must still be re-admitted even if
  // these follow-up writes fail; a later reload would skip the migration.
  try {
    const currentActiveCommunityId = storage.getItem(KUZZ_ACTIVE_COMMUNITY_KEY);
    if (
      legacyStorage.activeWorkspaceId &&
      (!currentActiveCommunityId || shouldWriteCommunities)
    ) {
      storage.setItem(
        KUZZ_ACTIVE_COMMUNITY_KEY,
        legacyStorage.activeWorkspaceId,
      );
    }

    for (const completion of legacyStorage.onboardingCompletions) {
      const key = `${KUZZ_ONBOARDING_COMPLETION_STORAGE_KEY_PREFIX}${completion.pubkey}`;
      if (storage.getItem(key) === null) {
        storage.setItem(key, completion.value);
      }
    }
  } catch (error) {
    console.warn("Failed to migrate legacy Sprout onboarding state.", error);
  }

  return introducedRelayUrls;
}

/**
 * Seed Kuzz localStorage from legacy Sprout WebKit localStorage before the app
 * renders providers that read community state. The native command reads the old
 * app identifier's WebKit SQLite database; this frontend step writes only when
 * Kuzz does not already have community state, except for the known broken
 * Sprout→Kuzz first-run handoff that created a single localhost community.
 */
export async function migrateLegacyCommunityStorageBeforeRender(): Promise<void> {
  if (typeof window === "undefined") {
    return;
  }

  migrateLegacyCommunityStorage(window.localStorage);
  // block/kuzz#5078 — read through the throw-safe accessor so a denied-storage
  // origin degrades to "no community state" instead of crashing pre-render.
  const currentCommunitiesRaw = getStorageItem(KUZZ_COMMUNITIES_KEY);
  const hasCurrentActiveCommunity = getStorageItem(KUZZ_ACTIVE_COMMUNITY_KEY);
  if (
    currentCommunitiesRaw &&
    hasCurrentActiveCommunity &&
    !hasOnlyLocalDevCommunity(currentCommunitiesRaw)
  ) {
    return;
  }

  let introducedRelayUrls: string[];
  try {
    introducedRelayUrls = applyLegacyCommunityStorage(
      await invokeTauri<LegacyCommunityStorageSnapshot>(
        "get_legacy_workspace_storage",
      ),
    );
  } catch (error) {
    console.warn("Failed to read legacy Sprout community storage.", error);
    return;
  }
  // A webview reload keeps the native process, so a relay removed earlier in
  // this session stays refused unless the migration re-admits it here.
  await Promise.all(
    introducedRelayUrls.map((relayUrl) =>
      readmitRelay(relayUrl).catch((error) => {
        console.error(
          "[communities] re-admitting local agents on a migrated relay failed:",
          error,
        );
      }),
    ),
  );
}
