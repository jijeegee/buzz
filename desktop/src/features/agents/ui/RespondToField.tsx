import * as React from "react";
import { AlertTriangle, Search, X } from "lucide-react";
import { mergeAllowlist } from "@/features/agents/lib/respondToAllowlist";
import { normalizePubkey, UNKNOWN_USER_LABEL } from "@/shared/lib/pubkey";
import { Skeleton } from "@/shared/ui/skeleton";
import { useIsArchivedPredicate } from "@/features/identity-archive/hooks";
import {
  useUserSearchQuery,
  useUsersBatchQuery,
} from "@/features/profile/hooks";
import { resolveUserLabel } from "@/features/profile/lib/identity";
import type { RespondToMode, UserSearchResult } from "@/shared/api/types";
import { Input } from "@/shared/ui/input";
import { UserAvatar } from "@/shared/ui/UserAvatar";
import {
  type AgentRunLocation,
  agentAccessWarningText,
} from "@/features/agents/lib/agentAccessWarning";
import { useAgentRunLocation } from "./AgentRunLocationContext";
import { PersonaDropdownField } from "./PersonaDropdownField";
import type { PersonaDropdownOption } from "./agentConfigOptions";

/**
 * Inbound author gate UI for create/edit agent dialogs.
 *
 * Dropdown:
 *   - Only me        (default; maps to `kuzz-acp --respond-to=owner-only`)
 *   - Anyone         (`--respond-to=anyone` — fully open agent)
 *   - Selected people (`--respond-to=allowlist`, plus the selected pubkeys as
 *                     `--respond-to-allowlist`)
 *
 * `nobody` is intentionally not surfaced — it pairs with a heartbeat-only
 * setup that has no meaningful GUI use case.
 *
 * Anyone and Selected people both share the host's access with someone other
 * than the owner, so both render the persistent warning; only the audience
 * phrase differs. It leads with the audience so it reads as a warning rather
 * than an explanation, and stays one sentence — Only me already owns the line
 * below the control.
 *
 * The line below Only me says "Only you and your agents", because the harness
 * gate admits the owner and every verified same-owner agent, not the owner
 * alone (see `managed_agents/access_policy.rs`). The dropdown label stays
 * "Only me": it is the audience the user picks, and it has meant this since
 * before agents could instruct each other.
 *
 * Which machine and stakes it names follow the optional `runLocation` prop, and
 * an unknown location falls back to the local wording rather than hedging with
 * "computer or server" — see `lib/agentAccessWarning.ts` for the copy and the
 * reasoning.
 *
 * Validation is duplicated lightly here for inline UX feedback only; the
 * authoritative validator is `validate_respond_to_allowlist` in
 * `desktop/src-tauri/src/managed_agents/types.rs`.
 */

function formatSearchUserName(user: UserSearchResult) {
  return (
    user.displayName?.trim() || user.nip05Handle?.trim() || UNKNOWN_USER_LABEL
  );
}

function formatSearchUserSecondary(user: UserSearchResult) {
  const displayName = user.displayName?.trim();
  const nip05Handle = user.nip05Handle?.trim();
  if (displayName && nip05Handle) {
    return nip05Handle;
  }
  return null;
}

const RESPOND_TO_OPTIONS: PersonaDropdownOption[] = [
  { label: "Only me (default)", value: "owner-only" },
  { label: "Anyone", value: "anyone" },
  { label: "Selected people", value: "allowlist" },
];

export const OWNER_ONLY_ACCESS_DISABLED_REASON =
  "This build disallows changing this setting.";

export function CreateAgentRespondToField({
  mode,
  allowlist,
  onModeChange,
  onAllowlistChange,
  ownerPubkey,
  disabled,
  disabledReason,
  variant,
  runLocation,
}: {
  mode: RespondToMode;
  allowlist: string[];
  onModeChange: (mode: RespondToMode) => void;
  onAllowlistChange: (allowlist: string[]) => void;
  /**
   * The agent's own owner pubkey, when known (Edit dialog). The owner is
   * always implicitly included by the harness — surfaced here as a small
   * informational note so it doesn't look like a missing entry.
   */
  ownerPubkey?: string | null;
  disabled?: boolean;
  /** Explanation shown when this access control is unavailable. */
  disabledReason?: string;
  /** When "persona", uses PersonaDropdownField styling to match the persona dialog. */
  variant?: "default" | "persona";
  /**
   * Where the agent's process runs, when the surface can tell. Omit or pass
   * `null` when it can't — the warning then uses the same "your computer"
   * wording as a local agent rather than hedging. Never synthesize a value.
   */
  runLocation?: AgentRunLocation | null;
}) {
  const [query, setQuery] = React.useState("");

  const deferredQuery = React.useDeferredValue(query.trim());
  const allowlistSet = React.useMemo(
    () => new Set(allowlist.map((p) => p.toLowerCase())),
    [allowlist],
  );
  const userSearchQuery = useUserSearchQuery(deferredQuery, {
    enabled: mode === "allowlist" && deferredQuery.length > 0,
    limit: 8,
  });
  const isArchivedDiscovery = useIsArchivedPredicate();
  const searchResults = React.useMemo(
    () =>
      (userSearchQuery.data ?? []).filter(
        (user) =>
          !allowlistSet.has(user.pubkey.toLowerCase()) &&
          !isArchivedDiscovery(user.pubkey),
      ),
    [allowlistSet, isArchivedDiscovery, userSearchQuery.data],
  );

  function handleAddSearchResult(user: UserSearchResult) {
    onAllowlistChange(mergeAllowlist(allowlist, [user.pubkey]));
    setQuery("");
  }

  function handleRemove(pubkey: string) {
    onAllowlistChange(
      allowlist.filter((p) => p.toLowerCase() !== pubkey.toLowerCase()),
    );
  }

  const isPersonaVariant = variant === "persona";

  // An explicit prop wins; otherwise inherit from the dialog subtree. Surfaces
  // inside AgentDialog get it from context (see AgentRunLocationContext for
  // why), standalone ones like EditRespondToDialog pass the prop.
  const inheritedRunLocation = useAgentRunLocation();
  const warningText = agentAccessWarningText(
    mode,
    runLocation ?? inheritedRunLocation,
  );

  // Rendered in two positions: directly below the selector for Anyone, but
  // after the people picker for Selected people, so it never sits between the
  // user and the selection they came here to make.
  const accessWarning = warningText ? (
    <div
      className="flex items-start gap-2 rounded-xl border border-warning/30 bg-warning-bg px-3 py-2.5"
      data-testid="agent-access-warning"
    >
      <AlertTriangle
        aria-hidden="true"
        className="mt-0.5 h-4 w-4 shrink-0 text-warning"
      />
      <p aria-live="polite" className="text-xs leading-5 text-warning">
        {warningText}
      </p>
    </div>
  ) : null;

  return (
    <div className="space-y-2" data-testid="agent-respond-to">
      <label
        className={
          isPersonaVariant
            ? "text-sm font-medium text-foreground"
            : "text-sm font-medium"
        }
        htmlFor="agent-respond-to"
      >
        Who can send instructions
      </label>
      {isPersonaVariant ? (
        <PersonaDropdownField
          disabled={disabled}
          id="agent-respond-to"
          onValueChange={(value) => onModeChange(value as RespondToMode)}
          options={RESPOND_TO_OPTIONS}
          placeholder="Only me (default)"
          value={mode}
        />
      ) : (
        <select
          className="flex h-9 w-full rounded-md border border-input bg-background px-3 py-2 text-sm shadow-xs"
          data-testid="agent-respond-to-select"
          disabled={disabled}
          id="agent-respond-to"
          onChange={(e) => onModeChange(e.target.value as RespondToMode)}
          value={mode}
        >
          {RESPOND_TO_OPTIONS.map((option) => (
            <option key={option.value} value={option.value}>
              {option.label}
            </option>
          ))}
        </select>
      )}
      {disabledReason ? (
        <p
          className="text-xs text-muted-foreground"
          data-testid="agent-respond-to-disabled-reason"
        >
          {disabledReason}
        </p>
      ) : null}
      {mode === "anyone" ? accessWarning : null}
      {mode === "owner-only" ? (
        <p className="text-xs text-muted-foreground">
          Only you and your agents can send instructions.
        </p>
      ) : null}
      {mode === "allowlist" ? (
        <AllowlistPicker
          allowlist={allowlist}
          deferredQuery={deferredQuery}
          disabled={disabled}
          onAddSearchResult={handleAddSearchResult}
          onQueryChange={setQuery}
          onRemove={handleRemove}
          ownerPubkey={ownerPubkey ?? null}
          query={query}
          searchError={
            userSearchQuery.error instanceof Error
              ? userSearchQuery.error.message
              : null
          }
          searchIsLoading={userSearchQuery.isLoading}
          searchResults={searchResults}
          variant={isPersonaVariant ? "persona" : "default"}
        />
      ) : null}
      {mode === "allowlist" ? accessWarning : null}
    </div>
  );
}

function AllowlistPicker({
  allowlist,
  deferredQuery,
  disabled,
  onAddSearchResult,
  onQueryChange,
  onRemove,
  ownerPubkey,
  query,
  searchError,
  searchIsLoading,
  searchResults,
  variant = "default",
}: {
  allowlist: string[];
  deferredQuery: string;
  disabled?: boolean;
  onAddSearchResult: (user: UserSearchResult) => void;
  onQueryChange: (value: string) => void;
  onRemove: (pubkey: string) => void;
  ownerPubkey: string | null;
  query: string;
  searchError: string | null;
  searchIsLoading: boolean;
  searchResults: UserSearchResult[];
  variant?: "default" | "persona";
}) {
  const isPersona = variant === "persona";
  const allowlistProfilesQuery = useUsersBatchQuery(allowlist);
  const allowlistProfiles = allowlistProfilesQuery.data?.profiles;
  const isAllowlistLoading =
    !allowlistProfiles && allowlistProfilesQuery.isLoading;

  return (
    <div
      className={
        isPersona
          ? "space-y-2.5"
          : "space-y-2.5 rounded-xl border border-border/80 bg-muted/15 p-3"
      }
      data-testid="agent-respond-to-allowlist"
    >
      {!isPersona ? (
        <div className="flex items-center justify-between gap-2">
          <span className="text-sm font-medium">Selected people</span>
          <span className="rounded-full bg-background px-2 py-1 text-2xs font-medium leading-none text-muted-foreground">
            {allowlist.length} selected
          </span>
        </div>
      ) : null}
      {!isPersona && ownerPubkey ? (
        <p className="text-xs text-muted-foreground">
          You can always use this agent. You don&apos;t need to add yourself.
        </p>
      ) : !isPersona ? (
        <p className="text-xs text-muted-foreground">
          You can always use this agent.
        </p>
      ) : null}
      <div className="rounded-lg border border-border/80 bg-background">
        <div className="flex items-center gap-2 px-2.5 py-2">
          <Search className="h-4 w-4 text-muted-foreground" />
          <Input
            className="h-auto border-0 px-0 py-0 shadow-none focus-visible:ring-0"
            data-testid="agent-respond-to-search"
            disabled={disabled}
            onChange={(event) => onQueryChange(event.target.value)}
            placeholder={isPersona ? "Search people" : "Search by name"}
            value={query}
          />
        </div>
        {allowlist.length > 0 ? (
          <div className="flex flex-wrap gap-1.5 border-t border-border/70 px-2.5 py-2">
            {allowlist.map((pubkey) => {
              const chipLabel = resolveUserLabel({
                profiles: allowlistProfiles,
                pubkey,
              });
              return (
                <div
                  className="inline-flex items-center gap-1.5 rounded-full border border-border/80 bg-muted/60 px-2.5 py-1 text-2xs leading-none"
                  data-testid={`agent-respond-to-chip-${pubkey}`}
                  key={pubkey}
                >
                  <UserAvatar
                    avatarUrl={
                      allowlistProfiles?.[normalizePubkey(pubkey)]?.avatarUrl ??
                      null
                    }
                    displayName={chipLabel}
                    size="xs"
                  />
                  {isAllowlistLoading ? (
                    <Skeleton
                      className="h-3 w-16"
                      data-testid={`agent-respond-to-chip-loading-${pubkey}`}
                    />
                  ) : (
                    <span>{chipLabel}</span>
                  )}
                  <button
                    aria-label={`Remove ${chipLabel}`}
                    className="text-muted-foreground transition-colors hover:text-foreground"
                    disabled={disabled}
                    onClick={() => onRemove(pubkey)}
                    type="button"
                  >
                    <X className="h-4 w-4" />
                  </button>
                </div>
              );
            })}
          </div>
        ) : null}
        {deferredQuery.length > 0 ? (
          <div className="border-t border-border/70 px-2 py-2">
            {searchIsLoading ? (
              <p className="px-2 py-1 text-sm text-muted-foreground">
                Searching…
              </p>
            ) : searchResults.length > 0 ? (
              <div className="max-h-44 space-y-1 overflow-y-auto">
                {searchResults.map((result) => (
                  <button
                    className="flex w-full items-center justify-between rounded-md px-2.5 py-1.5 text-left transition-colors hover:bg-accent hover:text-accent-foreground"
                    data-testid={`agent-respond-to-result-${result.pubkey}`}
                    key={result.pubkey}
                    onClick={() => onAddSearchResult(result)}
                    type="button"
                  >
                    <div className="flex items-center gap-2 min-w-0">
                      <UserAvatar
                        avatarUrl={result.avatarUrl}
                        displayName={formatSearchUserName(result)}
                        shape={result.isAgent ? "squircle" : "circle"}
                        size="xs"
                      />
                      <div className="min-w-0">
                        <p className="truncate text-sm font-medium leading-5">
                          {formatSearchUserName(result)}
                        </p>
                        {formatSearchUserSecondary(result) ? (
                          <p className="truncate text-xs text-muted-foreground">
                            {formatSearchUserSecondary(result)}
                          </p>
                        ) : null}
                      </div>
                    </div>
                    <span className="text-xs text-muted-foreground">Add</span>
                  </button>
                ))}
              </div>
            ) : (
              <p className="px-2 py-1 text-sm text-muted-foreground">
                No matching users.
              </p>
            )}
          </div>
        ) : null}
      </div>
      {searchError ? (
        <p className="text-sm text-destructive">{searchError}</p>
      ) : null}
    </div>
  );
}
