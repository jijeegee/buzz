import type {
  TeamSnapshotImportMemberResult,
  TeamSnapshotImportResult,
} from "@/shared/api/tauriTeams";
import { t } from "@/shared/i18n";

// ── Import phase derivation ──────────────────────────────────────────────────

export type ImportPhase = "preview" | "confirming" | "result";

export function deriveImportPhase(
  result: TeamSnapshotImportResult | null,
  isConfirming: boolean,
): ImportPhase {
  return result !== null ? "result" : isConfirming ? "confirming" : "preview";
}

// ── Profile-sync failure aggregation ─────────────────────────────────────────

export function getProfileSyncFailures(
  members: TeamSnapshotImportMemberResult[],
): TeamSnapshotImportMemberResult[] {
  return members.filter((m) => m.profileSyncError !== null);
}

// ── Toast message derivation ─────────────────────────────────────────────────

export type ToastOutcome = {
  type: "notice" | "error";
  message: string;
};

export function deriveImportToast(
  result: TeamSnapshotImportResult,
): ToastOutcome {
  const memberCount = result.members.length;
  const totalMemoryErrors = result.members.reduce(
    (sum, m) => sum + m.memoryErrors.length,
    0,
  );
  const profileSyncFailureCount = getProfileSyncFailures(result.members).length;

  if (totalMemoryErrors > 0 || profileSyncFailureCount > 0) {
    const parts: string[] = [];
    if (totalMemoryErrors > 0) {
      parts.push(
        t("agents.teamImport.memoryErrors", { count: totalMemoryErrors }),
      );
    }
    if (profileSyncFailureCount > 0) {
      parts.push(
        t("agents.teamImport.profileErrors", {
          count: profileSyncFailureCount,
        }),
      );
    }
    return {
      type: "error",
      message:
        parts.length === 2
          ? t("agents.teamImport.importedWithTwoProblems", {
              name: result.team.name,
              first: parts[0] ?? "",
              second: parts[1] ?? "",
            })
          : t("agents.teamImport.importedWithProblem", {
              name: result.team.name,
              problem: parts[0] ?? "",
            }),
    };
  }
  return {
    type: "notice",
    message: t("agents.teamImport.importedNotice", {
      name: result.team.name,
      count: memberCount,
    }),
  };
}
