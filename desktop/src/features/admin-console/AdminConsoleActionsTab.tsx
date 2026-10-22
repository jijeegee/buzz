/**
 * Actions tab — ban, time out, or delete without a report.
 *
 * Review freezes the whole intent (origin, relay, signer, community, verb,
 * target, reason, duration, requestId). Confirm and every retry resend that
 * frozen intent, so the relay dedupes on the same requestId. Review and
 * Confirm share one in-flight lock, so a late Review can never replace an
 * intent that is frozen or being sent. The native layer
 * mints a fresh NIP-98 signature per attempt and refuses the send if the
 * active relay or signer moved. The panel remounts this tab on an identity or
 * origin change, which drops any frozen intent.
 *
 * The community defaults to the active relay's host (read-only, with a
 * Change escape hatch), and members are picked by name on that relay or by a
 * pasted npub/hex key anywhere.
 */

import { useDeferredValue, useRef, useState } from "react";
import { toast } from "sonner";
import { classifyKeyImportInput } from "@/features/onboarding/lib/keyImportInput";
import { useCommunities } from "@/features/communities/useCommunities";
import {
  useUserProfileQuery,
  useUserSearchQuery,
} from "@/features/profile/hooks";
import { ProfileAvatar } from "@/features/profile/ui/ProfileAvatar";
import { SelectedRecipientChip } from "@/features/profile/ui/SelectedRecipientChip";
import { getRelayWsUrl } from "@/shared/api/tauri";
import { type MessageKey, t as translate, useT } from "@/shared/i18n";
import type { UserSearchResult } from "@/shared/api/types";
import { parsePubkeyInput } from "@/shared/lib/nostrUtils";
import { truncateNpub } from "@/shared/lib/pubkey";
import { Button } from "@/shared/ui/button";
import { PubKey } from "@/shared/ui/PubKey";
import {
  directAdminAction,
  type AdminDirectAction,
  type AdminDirectIntent,
  communityHostFromRelayUrl,
  normalizeCommunityHost,
} from "./api";
import {
  adminErrorCode,
  adminErrorMessage,
  adminMutationBodyComplete,
  adminMutationNotSent,
  adminMutationRelayStatus,
  preserveRequestIdOnError,
  useAsyncLoad,
} from "./AdminConsolePanelHelpers";
import { reasonAudienceCopy } from "./AdminConsoleReportsTab";

const HEX64 = /^[0-9a-f]{64}$/;

const ACTION_LABELS: Record<AdminDirectAction, MessageKey> = {
  ban: "adminConsole.actions.banMember",
  timeout: "adminConsole.actions.timeOutMember",
  delete: "adminConsole.actions.deleteMessage",
};

function directErrorMessage(e: unknown): string {
  switch (adminErrorCode(e)) {
    case "target_is_staff":
      return translate("adminConsole.actions.errorTargetIsStaff");
    case "request_id_conflict":
      return translate("adminConsole.actions.errorRequestIdConflict");
    case null: {
      // A relay from before direct actions answers these routes with an empty
      // 404/405; any other uncoded body keeps its own text.
      const status = adminMutationRelayStatus(e);
      if (
        (status === 404 || status === 405) &&
        adminMutationBodyComplete(e) &&
        adminErrorMessage(e).trim() === "admin API error:"
      ) {
        return translate("adminConsole.actions.errorUnsupported");
      }
      return adminErrorMessage(e);
    }
    default:
      return adminErrorMessage(e);
  }
}

/** An `nsec` or NIP-49 backup, in either case. */
function isSecretKey(input: string): boolean {
  return classifyKeyImportInput(input.toLowerCase()) !== "unknown";
}

function memberLabel(user: UserSearchResult): string {
  return (
    user.displayName?.trim() ||
    user.nip05Handle?.trim() ||
    truncateNpub(user.pubkey)
  );
}

/** Client-side checks; the relay re-validates everything. */
function validate(
  action: AdminDirectAction,
  host: string,
  target: string,
  member: UserSearchResult | null,
  secs: string,
): string | null {
  if (!host.trim()) return translate("adminConsole.actions.errorHostRequired");
  if (action === "delete" && !HEX64.test(target.trim())) {
    return translate("adminConsole.actions.errorEventId");
  }
  if (action !== "delete" && !member) {
    return translate("adminConsole.actions.errorMemberRequired");
  }
  if (
    action === "timeout" &&
    !(Number.isInteger(Number(secs)) && Number(secs) > 0)
  ) {
    return translate("adminConsole.actions.errorDuration");
  }
  return null;
}

export function ActionsTab({
  canMutate,
  origin,
  pubkey,
}: {
  canMutate: boolean;
  origin: string;
  /** Active signer; frozen into the intent at review. */
  pubkey: string;
}) {
  const t = useT();
  const [action, setAction] = useState<AdminDirectAction>("ban");
  const { activeCommunity } = useCommunities();
  /** Operator-typed host; null while following the active community. */
  const [hostOverride, setHostOverride] = useState<string | null>(null);
  const [target, setTarget] = useState("");
  const [member, setMember] = useState<UserSearchResult | null>(null);
  /** Display-only name for the frozen target; never sent. */
  const [frozenName, setFrozenName] = useState<string | null>(null);
  const [reason, setReason] = useState("");
  const [secs, setSecs] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [frozen, setFrozen] = useState<AdminDirectIntent | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const inFlight = useRef(false);

  // Host of the active relay: the default target community. Reloads when the
  // desktop switches communities.
  const activeHostState = useAsyncLoad(
    async () => communityHostFromRelayUrl(await getRelayWsUrl()),
    [activeCommunity?.relayUrl ?? ""],
    0,
  );
  const activeHost =
    activeHostState.status === "ok" ? activeHostState.data : null;

  const editingHost = hostOverride !== null || !activeHost;
  const host = hostOverride ?? activeHost ?? "";
  // Name search queries the active relay, so it only finds members there.
  const normalizedHost = normalizeCommunityHost(host);
  const onActiveCommunity =
    activeHost !== null && normalizedHost === activeHost;
  // Profiles are per community: a new host drops the picked member.
  const editHost = (next: string) => {
    if (normalizeCommunityHost(next) !== normalizedHost) setMember(null);
    setHostOverride(next);
  };

  const handleReview = async () => {
    if (frozen || inFlight.current) return;
    const invalid = validate(action, host, target, member, secs);
    setError(invalid);
    if (invalid) return;
    inFlight.current = true;
    setSubmitting(true);
    try {
      const expectedRelay = await getRelayWsUrl();
      // Following the active community: derive the host from the very relay
      // URL being frozen, so the two can never disagree.
      const communityHost =
        hostOverride?.trim() ?? communityHostFromRelayUrl(expectedRelay);
      if (!communityHost) {
        setError(t("adminConsole.actions.errorHostRequired"));
        return;
      }
      const isMember = action !== "delete" && member;
      setFrozenName(
        isMember && (member.displayName || member.nip05Handle)
          ? memberLabel(member)
          : null,
      );
      setFrozen({
        origin,
        expectedRelay,
        expectedPubkey: pubkey,
        communityHost,
        action,
        target: isMember ? member.pubkey : target.trim(),
        requestId: crypto.randomUUID(),
        reason: reason.trim() || undefined,
        expirationSecs: action === "timeout" ? Number(secs) : undefined,
      });
    } catch (e) {
      setError(adminErrorMessage(e));
    } finally {
      inFlight.current = false;
      setSubmitting(false);
    }
  };

  const handleConfirm = async () => {
    if (!frozen || inFlight.current) return;
    inFlight.current = true;
    setSubmitting(true);
    setError(null);
    setPending(false);
    try {
      const result = await directAdminAction(frozen);
      if (result.state === "pending") {
        setPending(true);
      } else {
        toast.success(
          t("adminConsole.actions.doneToast", {
            action: t(ACTION_LABELS[frozen.action]),
          }),
        );
        setFrozen(null);
        setTarget("");
        setMember(null);
        setReason("");
      }
    } catch (e) {
      // Keep the frozen intent (same requestId) unless the relay definitively
      // rejected it before committing. A request-id conflict is final for
      // this id, and a refusal before sending (bad host, relay or signer
      // moved) repeats on every resend, so neither can ever succeed.
      if (
        !preserveRequestIdOnError(e) ||
        adminMutationNotSent(e) ||
        adminErrorCode(e) === "request_id_conflict"
      ) {
        setFrozen(null);
      }
      setError(directErrorMessage(e));
    } finally {
      inFlight.current = false;
      setSubmitting(false);
    }
  };

  const locked = frozen !== null || submitting || !canMutate;
  // A pasted key has no name yet; look it up where search would find it.
  const confirmProfile = useUserProfileQuery(
    frozen &&
      frozen.action !== "delete" &&
      !frozenName &&
      frozen.communityHost === activeHost
      ? frozen.target
      : undefined,
  );
  const confirmName =
    frozenName ?? confirmProfile.data?.displayName?.trim() ?? null;
  const input = (
    name: string,
    value: string,
    set: (v: string) => void,
    placeholder: string,
    extra = "",
    type = "text",
  ) => (
    <input
      className={`w-full rounded-md border border-border/60 bg-background px-2 py-1 text-xs ${extra}`}
      data-testid={`direct-${name}-input`}
      disabled={locked}
      onChange={(e) => set(e.target.value)}
      placeholder={placeholder}
      type={type}
      value={value}
    />
  );

  return (
    <div className="space-y-3" data-testid="actions-tab">
      <div className="flex gap-1.5">
        {(Object.keys(ACTION_LABELS) as AdminDirectAction[]).map((a) => (
          <Button
            data-testid={`direct-action-${a}`}
            disabled={locked}
            key={a}
            onClick={() => setAction(a)}
            size="sm"
            type="button"
            variant={a === action ? "default" : "outline"}
          >
            {t(ACTION_LABELS[a])}
          </Button>
        ))}
      </div>
      {editingHost ? (
        input("host", host, editHost, t("adminConsole.actions.hostPlaceholder"))
      ) : (
        <p
          className="flex items-center gap-2 text-xs"
          data-testid="direct-host"
        >
          <span>
            {t("adminConsole.actions.communityLabel", {
              host,
              code: (chunks) => <code>{chunks}</code>,
            })}
          </span>
          <Button
            className="h-auto p-0 text-xs"
            data-testid="direct-host-change"
            disabled={locked}
            onClick={() => setHostOverride(host)}
            size="sm"
            type="button"
            variant="link"
          >
            {t("adminConsole.actions.changeHost")}
          </Button>
        </p>
      )}
      {action === "delete" ? (
        input(
          "target",
          target,
          setTarget,
          t("adminConsole.actions.eventIdPlaceholder"),
          "font-mono",
        )
      ) : (
        <MemberPicker
          disabled={locked}
          key={normalizedHost}
          member={member}
          onChange={setMember}
          searchEnabled={onActiveCommunity}
        />
      )}
      {action === "timeout" &&
        input(
          "duration",
          secs,
          setSecs,
          t("adminConsole.actions.durationPlaceholder"),
          "",
          "number",
        )}
      {input(
        "reason",
        reason,
        setReason,
        t("adminConsole.reasonOptionalPlaceholder"),
      )}
      <p
        className="text-xs text-muted-foreground"
        data-testid="direct-reason-audience"
      >
        {reasonAudienceCopy(t, frozen?.action ?? action)}
      </p>
      {pending && (
        <p
          className="text-xs text-muted-foreground"
          data-testid="direct-pending"
        >
          {t("adminConsole.actions.pending")}
        </p>
      )}
      {error && (
        <p className="text-xs text-destructive" data-testid="direct-error">
          {error}
        </p>
      )}
      {frozen ? (
        <div
          className="space-y-2 rounded-md border border-border/60 px-3 py-2 text-xs"
          data-testid="direct-confirm"
        >
          <p>
            {t(
              frozen.expirationSecs
                ? "adminConsole.actions.confirmSummaryTimed"
                : "adminConsole.actions.confirmSummary",
              {
                action: frozen.action,
                host: frozen.communityHost,
                secs: frozen.expirationSecs ?? 0,
                target:
                  frozen.action === "delete" ? (
                    <code>{frozen.target}</code>
                  ) : (
                    <span data-testid="direct-confirm-member">
                      {confirmName ? `${confirmName} ` : ""}
                      <code>
                        {confirmName
                          ? `(${truncateNpub(frozen.target)})`
                          : truncateNpub(frozen.target)}
                      </code>
                    </span>
                  ),
                code: (chunks) => <code>{chunks}</code>,
              },
            )}
          </p>
          {frozen.action !== "delete" && (
            <PubKey
              pubkey={frozen.target}
              testId="direct-confirm-npub"
              variant="full"
            />
          )}
          <p data-testid="direct-confirm-reason">
            {t("adminConsole.actions.confirmReason", {
              reason: frozen.reason ?? t("adminConsole.actions.reasonNone"),
            })}
          </p>
          <div className="flex gap-1.5">
            <Button
              data-testid="direct-confirm-btn"
              disabled={!canMutate || submitting}
              onClick={() => void handleConfirm()}
              size="sm"
              type="button"
              variant="destructive"
            >
              {error || pending ? t("common.retry") : t("common.confirm")}
            </Button>
            <Button
              data-testid="direct-discard-btn"
              disabled={submitting}
              onClick={() => {
                setFrozen(null);
                setError(null);
                setPending(false);
              }}
              size="sm"
              type="button"
              variant="ghost"
            >
              {t("adminConsole.actions.discard")}
            </Button>
          </div>
        </div>
      ) : (
        <Button
          data-testid="direct-review-btn"
          disabled={!canMutate || submitting}
          onClick={() => void handleReview()}
          size="sm"
          type="button"
        >
          {t("adminConsole.actions.review")}
        </Button>
      )}
    </div>
  );
}

/**
 * Pick a ban/timeout target: search names on the active relay, or paste an
 * npub or hex key. Mirrors the Add Member dialog's picker.
 */
function MemberPicker({
  disabled,
  member,
  onChange,
  searchEnabled,
}: {
  disabled: boolean;
  member: UserSearchResult | null;
  onChange: (member: UserSearchResult | null) => void;
  /** Off when the target community is not the active relay's. */
  searchEnabled: boolean;
}) {
  const t = useT();
  const [query, setQuery] = useState("");
  const [inspecting, setInspecting] = useState(false);
  const deferred = useDeferredValue(query.trim());
  const parsed = parsePubkeyInput(deferred);
  // A pasted secret key must never leave the device as a search query.
  const secret = isSecretKey(query) || isSecretKey(deferred);
  const search = useUserSearchQuery(deferred, {
    enabled: searchEnabled && !secret && deferred.length > 0 && parsed === null,
    limit: 8,
  });
  const results: UserSearchResult[] = parsed
    ? [
        {
          pubkey: parsed,
          displayName: null,
          avatarUrl: null,
          nip05Handle: null,
          ownerPubkey: null,
          isAgent: false,
        },
      ]
    : searchEnabled && !secret
      ? (search.data ?? [])
      : [];

  const hint = !searchEnabled && (
    <p
      className="text-xs text-muted-foreground"
      data-testid="direct-member-search-hint"
    >
      {t("adminConsole.actions.memberSearchHint")}
    </p>
  );
  if (member) {
    return (
      <div className="space-y-1" data-testid="direct-member-selected">
        <SelectedRecipientChip
          disabled={disabled}
          inspectable
          inspectionOpen={inspecting}
          label={memberLabel(member)}
          onInspectionOpenChange={setInspecting}
          onRemove={() => onChange(null)}
          poofOnRemove={false}
          testIds={{
            chip: "direct-member-remove",
            name: "direct-member-inspect",
            pubkey: "direct-member-npub",
          }}
          user={member}
        />
        {hint}
      </div>
    );
  }
  return (
    <div className="space-y-1">
      <input
        className="w-full rounded-md border border-border/60 bg-background px-2 py-1 text-xs"
        data-testid="direct-member-input"
        disabled={disabled}
        onChange={(e) => setQuery(e.target.value)}
        placeholder={
          searchEnabled
            ? t("adminConsole.actions.memberSearchPlaceholder")
            : t("adminConsole.actions.memberKeyPlaceholder")
        }
        value={query}
      />
      {secret && (
        <p
          className="text-xs text-destructive"
          data-testid="direct-member-secret"
        >
          {t("adminConsole.actions.memberSecretKey")}
        </p>
      )}
      {hint}
      {results.length > 0 && (
        <div className="rounded-md border border-border/60" role="listbox">
          {results.map((user) => (
            <button
              className="flex w-full items-center gap-2 px-2 py-1.5 text-left text-xs hover:bg-muted/50"
              data-testid={`direct-member-result-${user.pubkey}`}
              disabled={disabled}
              key={user.pubkey}
              onClick={() => {
                onChange(user);
                setQuery("");
              }}
              role="option"
              type="button"
            >
              <ProfileAvatar
                mediaVariant="small"
                avatarUrl={user.avatarUrl}
                className="h-6 w-6 text-2xs shadow-none"
                iconClassName="h-3 w-3"
                label={memberLabel(user)}
                shape={user.isAgent ? "squircle" : "circle"}
              />
              <span className="min-w-0 flex-1 truncate">
                {memberLabel(user)}
              </span>
              {parsed ? (
                <span className="text-muted-foreground">
                  {t("adminConsole.actions.memberPublicKey")}
                </span>
              ) : (
                <PubKey
                  className="text-muted-foreground"
                  interactive={false}
                  pubkey={user.pubkey}
                />
              )}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
