import type { ProjectRepoUnavailableReason } from "./projectRepoAvailability";
import { t } from "@/shared/i18n";

export type ProjectGitErrorPresentation = {
  title: string;
  description: string;
};

function errorText(error: unknown) {
  if (error instanceof Error) return error.message.toLowerCase();
  return typeof error === "string" ? error.toLowerCase() : "";
}

function isGitHubUrl(cloneUrl: string | null | undefined) {
  try {
    return new URL(cloneUrl ?? "").hostname.toLowerCase() === "github.com";
  } catch {
    return false;
  }
}

export function projectCloneErrorPresentation(
  error: unknown,
  cloneUrl?: string | null,
  unavailableReason?: ProjectRepoUnavailableReason,
): ProjectGitErrorPresentation {
  const message = errorText(error);
  const github = isGitHubUrl(cloneUrl);

  if (unavailableReason === "access") {
    return {
      title: t("projects.repoAvailability.accessTitle"),
      description: t("projects.gitError.accessNeeded"),
    };
  }
  if (
    /\b(?:401|403)\b|authenticat|authoriz|permission denied|access denied|ssh certificate/.test(
      message,
    )
  ) {
    return {
      title: t("projects.gitError.authTitle"),
      description: github
        ? t("projects.gitError.authGitHub")
        : t("projects.repoAvailability.authDescription"),
    };
  }
  if (/\b404\b|repository not found|repository does not exist/.test(message)) {
    return {
      title: t("projects.gitError.notFoundTitle"),
      description: t("projects.gitError.notFoundDescription"),
    };
  }
  if (
    /timed? out|could not resolve host|failed to connect|connection (?:refused|reset)|network is unreachable|offline/.test(
      message,
    )
  ) {
    return {
      title: t("projects.gitError.networkTitle"),
      description: t("projects.gitError.networkDescription"),
    };
  }
  if (
    /already exists and is not an empty directory|destination path .* exists/.test(
      message,
    )
  ) {
    return {
      title: t("projects.gitError.folderExistsTitle"),
      description: t("projects.gitError.folderExistsDescription"),
    };
  }
  return {
    title: t("projects.gitError.cloneFailedTitle"),
    description: github
      ? t("projects.gitError.cloneFailedGitHub")
      : t("projects.gitError.cloneFailedOther"),
  };
}
