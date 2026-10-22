import type { RespondToMode } from "@/shared/api/types";
import { type MessageKey, t } from "@/shared/i18n";

/**
 * Who may command an agent. `respondTo` null/undefined means "unset", which
 * the harness treats as owner-only.
 */
export type RespondToAccess = {
  respondTo?: RespondToMode | null;
  respondToAllowlist?: readonly string[] | null;
};

const ACCESS_RANK: Record<RespondToMode, number> = {
  "owner-only": 0,
  allowlist: 1,
  anyone: 2,
};

const ACCESS_LABEL: Record<RespondToMode, MessageKey> = {
  "owner-only": "agents.respondTo.mode.ownerOnly",
  allowlist: "agents.respondTo.mode.allowlist",
  anyone: "agents.respondTo.mode.anyone",
};

function modeOf(access: RespondToAccess | null): RespondToMode {
  return access?.respondTo ?? "owner-only";
}

function allowlistOf(access: RespondToAccess | null): Set<string> {
  return new Set(
    (access?.respondToAllowlist ?? []).map((pubkey) =>
      pubkey.trim().toLowerCase(),
    ),
  );
}

/**
 * Whether `next` lets more people command the agent than `prev`.
 *
 * `prev` null means there is no saved definition yet (a draft that creates a
 * new agent), so the baseline is owner-only.
 */
export function isRespondToWidening(
  prev: RespondToAccess | null,
  next: RespondToAccess,
): boolean {
  const prevMode = modeOf(prev);
  const nextMode = modeOf(next);
  if (ACCESS_RANK[nextMode] !== ACCESS_RANK[prevMode]) {
    return ACCESS_RANK[nextMode] > ACCESS_RANK[prevMode];
  }
  if (nextMode !== "allowlist") return false;
  const prevAllowlist = allowlistOf(prev);
  return [...allowlistOf(next)].some((pubkey) => !prevAllowlist.has(pubkey));
}

/** Warning copy for a widening draft, or null when access does not widen. */
export function respondToWideningWarning(
  prev: RespondToAccess | null,
  next: RespondToAccess,
): string | null {
  if (!isRespondToWidening(prev, next)) return null;
  const prevMode = modeOf(prev);
  const nextMode = modeOf(next);
  if (prevMode === nextMode) {
    return t("agents.respondTo.wideningAdded", {
      mode: t(ACCESS_LABEL[nextMode]),
    });
  }
  return t("agents.respondTo.wideningChanged", {
    from: t(ACCESS_LABEL[prevMode]),
    to: t(ACCESS_LABEL[nextMode]),
  });
}
