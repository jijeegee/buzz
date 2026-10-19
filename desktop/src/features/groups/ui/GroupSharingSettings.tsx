import * as React from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Copy, RotateCcw } from "lucide-react";
import { toast } from "sonner";

import { FieldGroup } from "@/features/channels/ui/ChannelManagementSheetRows";
import { SettingsConfirmDialog } from "@/features/settings/ui/SettingsConfirmDialog";
import { writeTextToClipboard } from "@/shared/lib/clipboard";
import { useRelayOrigin } from "@/shared/lib/useRelayOrigin";
import { Button } from "@/shared/ui/button";
import { Input } from "@/shared/ui/input";
import { SegmentedControl } from "@/shared/ui/segmented-control";
import {
  isValidGroupHandle,
  normalizeGroupHandle,
  PUBLIC_ADDRESS_INVALID_TEXT,
  publicAddressErrorText,
  publicAddressPrefix,
} from "../groupLink";
import {
  type GroupInvitePolicy,
  type GroupSummary,
  groupsErrorStatus,
  groupsErrorText,
  resetGroupInviteLink,
  setGroupHandle,
  setGroupInvitePolicy,
} from "../groupsApi";
import {
  GROUPS_QUERY_ROOT,
  useGroupDetailQuery,
  useGroupInviteLinkQuery,
} from "../hooks";

const INVITE_POLICY_OPTIONS = [
  { value: "members", label: "All members" },
  { value: "admins", label: "Admins only" },
] as const;

function failureText(error: unknown, fallback: string): string {
  return groupsErrorText(error) || fallback;
}

/**
 * Group settings › Sharing: the public address (public groups), the invite
 * link, and who can invite (private groups). Owners and admins edit; members
 * see the invite link unless the group lets only admins invite.
 */
export function GroupSharingSettings({
  canManage,
  channelId,
  isMember,
  visibility,
}: {
  canManage: boolean;
  channelId: string;
  isMember: boolean;
  visibility: "open" | "private";
}) {
  const detailQuery = useGroupDetailQuery(channelId, isMember);
  const invalidate = useGroupsInvalidate(channelId);
  // Going private clears the address server-side: refetch on any switch.
  const lastVisibility = React.useRef(visibility);
  React.useEffect(() => {
    if (lastVisibility.current === visibility) return;
    lastVisibility.current = visibility;
    void invalidate();
  }, [invalidate, visibility]);
  const detail = detailQuery.data;
  const invitePolicy: GroupInvitePolicy = detail?.invitePolicy ?? "members";
  const canSeeInviteLink =
    isMember &&
    detail !== undefined &&
    (canManage || invitePolicy === "members");
  const showAddress = canManage && visibility === "open";
  const showInvitePolicy = canManage && visibility === "private";

  if (!isMember || (!showAddress && !canSeeInviteLink && !showInvitePolicy)) {
    return null;
  }

  return (
    <FieldGroup testId="group-sharing-settings" title="Sharing">
      {showAddress ? (
        <PublicAddressRow
          channelId={channelId}
          handle={detail?.handle ?? null}
        />
      ) : null}
      {canSeeInviteLink ? (
        <InviteLinkRow canReset={canManage} channelId={channelId} />
      ) : null}
      {showInvitePolicy && detail ? (
        <InvitePolicyRow channelId={channelId} policy={invitePolicy} />
      ) : null}
    </FieldGroup>
  );
}

function useGroupsInvalidate(channelId: string) {
  const queryClient = useQueryClient();
  return React.useCallback(
    () =>
      queryClient.invalidateQueries({
        predicate: (query) =>
          query.queryKey[0] === GROUPS_QUERY_ROOT[0] &&
          query.queryKey.includes(channelId),
      }),
    [channelId, queryClient],
  );
}

function PublicAddressRow({
  channelId,
  handle,
}: {
  channelId: string;
  handle: string | null;
}) {
  // The server's invite link carries its public web host (where /g/ pages
  // live); fall back to the community origin until it loads.
  const linkUrl = useGroupInviteLinkQuery(channelId, true).data?.url;
  const relayOrigin = useRelayOrigin();
  const prefix = publicAddressPrefix(linkUrl ?? relayOrigin);
  const invalidate = useGroupsInvalidate(channelId);
  const [draft, setDraft] = React.useState(handle ?? "");
  const [error, setError] = React.useState<string | null>(null);
  React.useEffect(() => setDraft(handle ?? ""), [handle]);

  const save = useMutation({
    mutationFn: (next: string | null) => setGroupHandle(channelId, next),
    onSuccess: (saved: GroupSummary) => {
      setDraft(saved.handle ?? "");
      toast.success(
        saved.handle ? "Public address saved" : "Public address removed",
      );
      void invalidate();
    },
    onError: (failure) =>
      setError(
        publicAddressErrorText(
          groupsErrorStatus(failure),
          groupsErrorText(failure),
        ) ?? failureText(failure, "Couldn't save the address."),
      ),
  });

  const normalized = normalizeGroupHandle(draft);
  const unchanged = normalized === (handle ?? "");

  const submit = (event: React.FormEvent) => {
    event.preventDefault();
    if (unchanged) return;
    if (normalized && !isValidGroupHandle(normalized)) {
      setError(PUBLIC_ADDRESS_INVALID_TEXT);
      return;
    }
    setError(null);
    save.mutate(normalized || null);
  };

  return (
    <form
      className="space-y-2 px-4 py-3"
      data-testid="group-public-address"
      onSubmit={submit}
    >
      <label className="text-sm font-medium" htmlFor="group-public-address">
        Public address
      </label>
      <div className="flex items-center gap-2">
        <div className="flex h-9 min-w-0 flex-1 items-center rounded-md border border-input bg-background pl-3 text-sm focus-within:ring-1 focus-within:ring-ring">
          <span
            className="shrink-0 truncate text-muted-foreground"
            data-testid="group-public-address-prefix"
          >
            {prefix}
          </span>
          <Input
            aria-invalid={error ? true : undefined}
            autoCapitalize="none"
            autoComplete="off"
            className="h-8 min-w-0 flex-1 border-0 bg-transparent px-0.5 shadow-none focus-visible:ring-0"
            data-testid="group-public-address-input"
            id="group-public-address"
            maxLength={33}
            onChange={(event) => {
              setDraft(event.target.value);
              setError(null);
            }}
            spellCheck={false}
            value={draft}
          />
        </div>
        <Button
          data-testid="group-public-address-save"
          disabled={unchanged || save.isPending}
          size="sm"
          type="submit"
        >
          {save.isPending ? "Saving..." : "Save"}
        </Button>
      </div>
      {error ? (
        <p
          className="text-sm text-destructive"
          data-testid="group-public-address-error"
          role="alert"
        >
          {error}
        </p>
      ) : null}
    </form>
  );
}

function InviteLinkRow({
  canReset,
  channelId,
}: {
  canReset: boolean;
  channelId: string;
}) {
  const linkQuery = useGroupInviteLinkQuery(channelId, true);
  const invalidate = useGroupsInvalidate(channelId);
  const [confirmOpen, setConfirmOpen] = React.useState(false);
  const url = linkQuery.data?.url ?? "";

  const reset = useMutation({
    mutationFn: () => resetGroupInviteLink(channelId),
    onSuccess: () => void invalidate(),
    onError: (failure) =>
      toast.error(failureText(failure, "Couldn't reset the link.")),
  });

  async function copy() {
    if (!url) return;
    await writeTextToClipboard(url);
    toast.success("Link copied");
  }

  return (
    <div className="space-y-2 px-4 py-3" data-testid="group-invite-link">
      <p className="text-sm font-medium">Invite link</p>
      <p
        className="truncate font-mono text-sm text-muted-foreground"
        data-testid="group-invite-link-url"
        title={url}
      >
        {linkQuery.isLoading
          ? "Loading..."
          : linkQuery.isError
            ? "Couldn't load the link."
            : url}
      </p>
      <div className="flex flex-wrap gap-2">
        <Button
          data-testid="group-invite-link-copy"
          disabled={!url}
          onClick={() => void copy()}
          size="sm"
          type="button"
          variant="outline"
        >
          <Copy className="h-4 w-4" />
          Copy link
        </Button>
        {canReset ? (
          <Button
            data-testid="group-invite-link-reset"
            disabled={!url || reset.isPending}
            onClick={() => setConfirmOpen(true)}
            size="sm"
            type="button"
            variant="outline"
          >
            <RotateCcw className="h-4 w-4" />
            Reset link
          </Button>
        ) : null}
      </div>
      <SettingsConfirmDialog
        confirmLabel="Reset link"
        description="The old link will stop working."
        onConfirm={() => reset.mutate()}
        onOpenChange={setConfirmOpen}
        open={confirmOpen}
        testId="group-invite-link-reset-dialog"
        title="Reset link?"
      />
    </div>
  );
}

function InvitePolicyRow({
  channelId,
  policy,
}: {
  channelId: string;
  policy: GroupInvitePolicy;
}) {
  const invalidate = useGroupsInvalidate(channelId);
  const [value, setValue] = React.useState(policy);
  React.useEffect(() => setValue(policy), [policy]);
  const save = useMutation({
    mutationFn: (next: GroupInvitePolicy) =>
      setGroupInvitePolicy(channelId, next),
    onSuccess: () => void invalidate(),
    onError: (failure) => {
      setValue(policy);
      toast.error(failureText(failure, "Couldn't change this setting."));
    },
  });

  return (
    <div
      className="flex min-h-16 items-center justify-between gap-3 px-4 py-3"
      data-testid="group-invite-policy"
    >
      <p className="text-sm font-medium">Who can invite</p>
      <SegmentedControl
        disabled={save.isPending}
        legend="Who can invite"
        onValueChange={(next) => {
          if (next === value) return;
          setValue(next);
          save.mutate(next);
        }}
        optionTestIdPrefix="group-invite-policy"
        options={INVITE_POLICY_OPTIONS}
        size="compact"
        testId="group-invite-policy-control"
        value={value}
      />
    </div>
  );
}
