import { invokeTauri } from "@/shared/api/tauri";
import type {
  KnownProfile,
  Profile,
  UpdateProfileInput,
  UserProfileSummary,
  UserSearchPage,
  UserSearchResult,
  UsersBatchResponse,
} from "@/shared/api/types";

type RawProfile = {
  pubkey: string;
  display_name: string | null;
  avatar_url: string | null;
  about: string | null;
  nip05_handle: string | null;
  owner_pubkey: string | null;
  has_profile_event?: boolean;
};

type RawUserProfileSummary = Omit<RawProfile, "pubkey" | "about"> & {
  name?: string | null;
  is_agent?: boolean;
  created_at?: number;
  event_id?: string;
};

type RawUsersBatchResponse = {
  profiles: Record<string, RawUserProfileSummary>;
  missing: string[];
  unchanged?: string[];
};

type RawUserSearchResult = RawUserProfileSummary & { pubkey: string };

type RawSearchUsersResponse = {
  users: RawUserSearchResult[];
  next_cursor?: string | null;
};

function fromRawProfile(profile: RawProfile): Profile {
  return {
    pubkey: profile.pubkey,
    displayName: profile.display_name,
    avatarUrl: profile.avatar_url,
    about: profile.about,
    nip05Handle: profile.nip05_handle,
    ownerPubkey: profile.owner_pubkey,
    hasProfileEvent: profile.has_profile_event ?? false,
  };
}

function fromRawUserProfileSummary(
  profile: RawUserProfileSummary,
): UserProfileSummary {
  return {
    displayName: profile.display_name,
    name: profile.name ?? null,
    avatarUrl: profile.avatar_url,
    nip05Handle: profile.nip05_handle,
    ownerPubkey: profile.owner_pubkey,
    isAgent: profile.is_agent ?? false,
    ...(typeof profile.created_at === "number" &&
      profile.created_at > 0 && { createdAt: profile.created_at }),
    ...(typeof profile.event_id === "string" &&
      /^[0-9a-f]{64}$/.test(profile.event_id) && { eventId: profile.event_id }),
  };
}

function fromRawUserSearchResult(user: RawUserSearchResult): UserSearchResult {
  return {
    pubkey: user.pubkey,
    displayName: user.display_name,
    avatarUrl: user.avatar_url,
    nip05Handle: user.nip05_handle,
    ownerPubkey: user.owner_pubkey,
    isAgent: user.is_agent ?? false,
  };
}

export async function getProfile(): Promise<Profile> {
  const profile = await invokeTauri<RawProfile>("get_profile");
  return fromRawProfile(profile);
}

export async function updateProfile(
  input: UpdateProfileInput,
): Promise<Profile> {
  const profile = await invokeTauri<RawProfile>("update_profile", input);
  return fromRawProfile(profile);
}

export async function updateProfileAtRelay(input: {
  relayUrl: string;
  expectedPubkey: string;
  expectedAvatarUrl: string | null;
  avatarUrl: string;
}): Promise<Profile> {
  const profile = await invokeTauri<RawProfile>(
    "update_profile_at_relay",
    input,
  );
  return fromRawProfile(profile);
}

export async function getUserProfile(pubkey?: string): Promise<Profile> {
  const profile = await invokeTauri<RawProfile>("get_user_profile", { pubkey });
  return fromRawProfile(profile);
}

/**
 * Reads other people's profiles. `known` holds the profiles already held:
 * only what changed since their `verifiedAt` is fetched, and one with
 * nothing newer comes back in `unchanged` instead of `missing`. Returned
 * profiles are stamped `verifiedAt` now.
 */
export async function getUsersBatch(
  pubkeys: string[],
  known?: Record<string, KnownProfile>,
): Promise<UsersBatchResponse> {
  const verifiedAt = Math.floor(Date.now() / 1000);
  const response = await invokeTauri<RawUsersBatchResponse>("get_users_batch", {
    pubkeys,
    ...(known && Object.keys(known).length > 0 && { known }),
  });

  return {
    profiles: Object.fromEntries(
      Object.entries(response.profiles).map(([pubkey, profile]) => [
        pubkey,
        { ...fromRawUserProfileSummary(profile), verifiedAt },
      ]),
    ),
    missing: response.missing,
    ...(response.unchanged?.length ? { unchanged: response.unchanged } : {}),
  };
}

export async function searchUsers(
  query: string,
  limit = 8,
  cursor?: string | null,
): Promise<UserSearchPage> {
  const response = await invokeTauri<RawSearchUsersResponse>("search_users", {
    query,
    limit,
    cursor: cursor ?? null,
  });
  return {
    users: response.users.map(fromRawUserSearchResult),
    nextCursor: response.next_cursor ?? null,
  };
}
