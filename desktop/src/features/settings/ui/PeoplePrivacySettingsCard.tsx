import * as React from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Mail } from "lucide-react";
import { toast } from "sonner";

import {
  useBlockedPeopleQuery,
  useUnblockPersonMutation,
} from "@/features/people/BlockedPeopleContext";
import { GroupAddPolicySetting } from "@/features/groups/ui/GroupAddPolicySetting";
import { useUsersBatchQuery } from "@/features/profile/hooks";
import { ProfileAvatar } from "@/features/profile/ui/ProfileAvatar";
import { useIdentityQuery } from "@/shared/api/hooks";
import {
  getPeopleSettings,
  isNotSignedIn,
  type PeopleSettings,
  setDiscoverable,
  setKuzzId,
} from "@/shared/api/people";
import { CONTACT_EMAIL, CONTACT_MAILTO } from "@/shared/lib/kuzzLinks";
import { Button } from "@/shared/ui/button";
import { Input } from "@/shared/ui/input";
import { Switch } from "@/shared/ui/switch";
import { kuzzIdError, normalizeKuzzId } from "../lib/kuzzId";
import { SettingsOptionGroup, SettingsOptionRow } from "./SettingsOptionGroup";

export const peopleSettingsQueryKey = (pubkey: string) =>
  ["people-settings", pubkey.toLowerCase()] as const;

function errorText(error: unknown, fallback: string): string {
  if (error instanceof Error && error.message) return error.message;
  if (typeof error === "string" && error) return error;
  return fallback;
}

/**
 * Settings › Profile: how others find me (Kuzz ID, "find me by ID or email"),
 * the people I blocked, and how to reach us.
 */
export function PeoplePrivacySettingsCard() {
  const identityQuery = useIdentityQuery();
  const pubkey = identityQuery.data?.pubkey;
  return (
    <div className="mt-12 space-y-12" data-testid="settings-people-privacy">
      <FindingYouGroup pubkey={pubkey} />
      <GroupAddPolicySetting />
      <BlockedUsersGroup pubkey={pubkey} />
      <SettingsOptionGroup title="Help">
        <SettingsOptionRow>
          <div className="min-w-0">
            <p className="text-sm font-medium">Contact us</p>
            <p
              className="text-sm font-normal text-muted-foreground/70"
              data-settings-subcopy
            >
              Questions, problems or reports: {CONTACT_EMAIL}
            </p>
          </div>
          <Button
            data-testid="settings-contact-us"
            onClick={() =>
              void openUrl(CONTACT_MAILTO).catch(() =>
                toast.error(`Write to us at ${CONTACT_EMAIL}`),
              )
            }
            size="sm"
            type="button"
            variant="outline"
          >
            <Mail className="h-4 w-4" />
            Email us
          </Button>
        </SettingsOptionRow>
      </SettingsOptionGroup>
    </div>
  );
}

function FindingYouGroup({ pubkey }: { pubkey: string | undefined }) {
  const queryClient = useQueryClient();
  const queryKey = peopleSettingsQueryKey(pubkey ?? "");
  const settingsQuery = useQuery<PeopleSettings>({
    enabled: Boolean(pubkey),
    queryKey,
    queryFn: getPeopleSettings,
    staleTime: 60_000,
    retry: false,
  });
  const settings = settingsQuery.data;
  const [draft, setDraft] = React.useState("");
  const [idError, setIdError] = React.useState<string | null>(null);
  const savedId = settings?.kuzzId ?? "";

  React.useEffect(() => setDraft(savedId), [savedId]);

  const saveId = useMutation({
    mutationFn: (input: string) => setKuzzId(input),
    onSuccess: (saved) => {
      queryClient.setQueryData<PeopleSettings>(queryKey, (current) =>
        current ? { ...current, kuzzId: saved } : current,
      );
      setDraft(saved ?? "");
      toast.success(saved ? "Kuzz ID saved" : "Kuzz ID removed");
    },
    onError: (error) =>
      setIdError(errorText(error, "Couldn't save your Kuzz ID")),
  });
  const saveDiscoverable = useMutation({
    mutationFn: (next: boolean) => setDiscoverable(next),
    onMutate: (next) => {
      const previous = queryClient.getQueryData<PeopleSettings>(queryKey);
      queryClient.setQueryData<PeopleSettings>(queryKey, (current) =>
        current ? { ...current, discoverable: next } : current,
      );
      return { previous };
    },
    onError: (error, _next, context) => {
      queryClient.setQueryData(queryKey, context?.previous);
      toast.error(errorText(error, "Couldn't change this setting"));
    },
  });

  const submit = (event: React.FormEvent) => {
    event.preventDefault();
    const problem = kuzzIdError(draft);
    setIdError(problem);
    if (problem || normalizeKuzzId(draft) === savedId) return;
    saveId.mutate(normalizeKuzzId(draft));
  };
  // Only a missing sign-in asks to sign in; anything else can be retried.
  const notSignedIn =
    settingsQuery.isError && isNotSignedIn(settingsQuery.error);
  const loadFailed = settingsQuery.isError && !notSignedIn;

  return (
    <SettingsOptionGroup
      description="Friends can add you by your Kuzz ID or email address."
      title="Finding you"
    >
      <SettingsOptionRow>
        <form className="w-full space-y-2" onSubmit={submit}>
          <label className="text-sm font-medium" htmlFor="settings-kuzz-id">
            Kuzz ID
          </label>
          <div className="flex items-center gap-2">
            <span aria-hidden className="text-sm text-muted-foreground">
              @
            </span>
            <Input
              aria-describedby="settings-kuzz-id-help"
              aria-invalid={idError ? true : undefined}
              autoCapitalize="none"
              autoComplete="off"
              className="h-8 max-w-xs"
              data-testid="settings-kuzz-id"
              disabled={!settings || saveId.isPending}
              id="settings-kuzz-id"
              maxLength={33}
              onChange={(event) => {
                setDraft(event.target.value);
                setIdError(null);
              }}
              placeholder="your_id"
              spellCheck={false}
              value={draft}
            />
            <Button
              data-testid="settings-kuzz-id-save"
              disabled={
                !settings ||
                saveId.isPending ||
                normalizeKuzzId(draft) === savedId
              }
              size="sm"
              type="submit"
            >
              Save
            </Button>
          </div>
          <p
            className={
              idError
                ? "text-sm text-destructive"
                : "text-sm font-normal text-muted-foreground/70"
            }
            data-settings-subcopy={idError ? undefined : true}
            data-testid="settings-kuzz-id-help"
            id="settings-kuzz-id-help"
            role={idError ? "alert" : undefined}
          >
            {idError ??
              (notSignedIn
                ? "Sign in with Google to set your Kuzz ID."
                : loadFailed
                  ? "Couldn't load your Kuzz ID."
                  : "3 to 32 lowercase letters, numbers or _.")}
          </p>
          {loadFailed ? (
            <Button
              data-testid="settings-people-retry"
              disabled={settingsQuery.isFetching}
              onClick={() => void settingsQuery.refetch()}
              size="sm"
              type="button"
              variant="outline"
            >
              Try again
            </Button>
          ) : null}
        </form>
      </SettingsOptionRow>
      <SettingsOptionRow>
        <div className="min-w-0">
          <label
            className="text-sm font-medium"
            htmlFor="settings-discoverable"
          >
            Let people find me by ID or email
          </label>
          <p
            className="text-sm font-normal text-muted-foreground/70"
            data-settings-subcopy
          >
            When this is off, only people in your groups can find you.
          </p>
        </div>
        <Switch
          checked={settings?.discoverable ?? true}
          data-testid="settings-discoverable"
          disabled={!settings || saveDiscoverable.isPending}
          id="settings-discoverable"
          onCheckedChange={(checked) => saveDiscoverable.mutate(checked)}
        />
      </SettingsOptionRow>
    </SettingsOptionGroup>
  );
}

function BlockedUsersGroup({ pubkey }: { pubkey: string | undefined }) {
  const blockedQuery = useBlockedPeopleQuery(pubkey);
  const unblock = useUnblockPersonMutation(pubkey);
  const pubkeys = React.useMemo(
    () =>
      (blockedQuery.data ?? []).map((person) => person.pubkey.toLowerCase()),
    [blockedQuery.data],
  );
  const profilesQuery = useUsersBatchQuery(pubkeys, {
    enabled: pubkeys.length > 0,
  });
  const profiles = profilesQuery.data?.profiles;

  return (
    <SettingsOptionGroup
      description="Blocked people can't reach you, and their messages in shared groups are hidden."
      title="Blocked users"
    >
      {blockedQuery.isLoading ? (
        <SettingsOptionRow>
          <p className="text-sm text-muted-foreground">Loading…</p>
        </SettingsOptionRow>
      ) : blockedQuery.isError ? (
        <SettingsOptionRow>
          <p className="text-sm text-muted-foreground" role="status">
            Couldn't load the people you blocked.
          </p>
          <Button
            onClick={() => void blockedQuery.refetch()}
            size="sm"
            type="button"
            variant="outline"
          >
            Try again
          </Button>
        </SettingsOptionRow>
      ) : pubkeys.length === 0 ? (
        <SettingsOptionRow data-testid="settings-blocked-empty">
          <p className="text-sm text-muted-foreground">
            You haven't blocked anyone.
          </p>
        </SettingsOptionRow>
      ) : (
        pubkeys.map((key) => {
          const profile = profiles?.[key];
          const name =
            profile?.displayName?.trim() ||
            profile?.nip05Handle?.trim() ||
            "Unknown";
          return (
            <SettingsOptionRow
              data-testid={`settings-blocked-${key}`}
              key={key}
            >
              <div className="flex min-w-0 items-center gap-3">
                <ProfileAvatar
                  avatarUrl={profile?.avatarUrl ?? null}
                  className="h-8 w-8 text-xs shadow-none"
                  iconClassName="h-4 w-4"
                  label={name}
                  shape="circle"
                />
                <span className="truncate text-sm font-medium">{name}</span>
              </div>
              <Button
                aria-label={`Unblock ${name}`}
                data-testid={`settings-unblock-${key}`}
                disabled={unblock.isPending && unblock.variables === key}
                onClick={() =>
                  unblock.mutate(key, {
                    onSuccess: () => toast.success(`${name} is unblocked`),
                    onError: (error) =>
                      toast.error(errorText(error, "Couldn't unblock")),
                  })
                }
                size="sm"
                type="button"
                variant="outline"
              >
                Unblock
              </Button>
            </SettingsOptionRow>
          );
        })
      )}
    </SettingsOptionGroup>
  );
}
