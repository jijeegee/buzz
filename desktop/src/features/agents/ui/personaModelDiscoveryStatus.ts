import { t } from "@/shared/i18n";
export type PersonaModelDiscoveryStatus = {
  message: string;
  tone: "muted" | "warning";
};

function errorMessage(error: unknown): string {
  if (error instanceof Error) {
    return error.message;
  }
  if (typeof error === "string") {
    return error;
  }
  try {
    return JSON.stringify(error);
  } catch {
    return t("agents.modelStatus.unknownError");
  }
}

function providerObjectLabel(provider: string): string {
  switch (provider.trim()) {
    case "anthropic":
      return "Anthropic";
    case "openai":
      return "OpenAI";
    case "openai-compat":
      return "OpenAI-compatible";
    default:
      return provider.trim();
  }
}

function isEmptySharedComputeError(message: string): boolean {
  const normalized = message.toLowerCase();
  return (
    normalized.includes("shared compute status is not published") ||
    normalized.includes("shared compute serving members are available") ||
    normalized.includes("shared compute models are available") ||
    normalized.includes("no live member is serving") ||
    normalized.includes("requires a live serving member")
  );
}

export function formatModelDiscoveryErrorStatus(
  error: unknown,
  provider: string,
  agentLabel?: string,
): PersonaModelDiscoveryStatus | null {
  const message = errorMessage(error);

  if (provider.trim() === "relay-mesh") {
    if (message.includes("waiting for the current member roster")) {
      return {
        message: t("agents.modelStatus.loadingMembers"),
        tone: "warning",
      };
    }

    if (isEmptySharedComputeError(message)) {
      return {
        message: t("agents.modelStatus.noSharedCompute"),
        tone: "warning",
      };
    }

    if (message.includes("shared compute is not available in this build")) {
      return {
        message: t("agents.modelStatus.sharedComputeUnsupported"),
        tone: "warning",
      };
    }

    if (message.includes("shared compute status is malformed")) {
      return {
        message: t("agents.modelStatus.sharedComputeMalformed"),
        tone: "warning",
      };
    }

    return {
      message: t("agents.modelStatus.sharedComputeFailed"),
      tone: "warning",
    };
  }

  // Spec-reserved auth error text (agent-client-protocol ErrorCode::AuthRequired),
  // surfaced verbatim through kuzz-acp's stderr — generic across conformant
  // harnesses (e.g. cursor-agent when not signed in). Match the message text,
  // NOT code -32000: that code is also the catch-all fallback for unclassified
  // errors, so matching it would swallow unrelated failures into "sign in".
  if (message.toLowerCase().includes("authentication required")) {
    const label = agentLabel?.trim();
    return {
      message: label
        ? t("agents.modelStatus.signInRequired", { name: label })
        : t("agents.modelStatus.signInRequiredUnnamed"),
      tone: "warning",
    };
  }

  if (message.includes("ANTHROPIC_API_KEY required")) {
    return {
      message: t("agents.modelStatus.anthropicKey"),
      tone: "warning",
    };
  }

  if (message.includes("OPENAI_COMPAT_API_KEY required")) {
    return {
      message: t("agents.modelStatus.openaiKey"),
      tone: "warning",
    };
  }

  if (
    message.includes("DATABRICKS_HOST required") ||
    message.includes("DATABRICKS_MODEL required") ||
    message.includes("KUZZ_AGENT_PROVIDER is required")
  ) {
    return null;
  }

  // Databricks transparent auth (agent_models_databricks.rs). The backend
  // launches the browser OAuth flow itself from every discovery surface, so
  // these are terminal outcomes the user should see, not raw error text.
  // Matched on the stable error strings the backend emits (string matching is
  // this file's convention until typed error codes arrive).
  const databricksStatus = formatDatabricksAuthStatus(message);
  if (databricksStatus !== null) {
    return databricksStatus;
  }

  const providerLabel = providerObjectLabel(provider);
  return {
    message: providerLabel
      ? t("agents.modelStatus.builtInOptions", { provider: providerLabel })
      : t("agents.modelStatus.builtInOptionsUnnamed"),
    tone: "warning",
  };
}

/**
 * Maps the terminal Databricks sign-in states to user-facing guidance, or null
 * when the error is not a Databricks sign-in outcome. "Sign-in required" is a
 * quiet muted note (a passive surface hit its cooldown, or an unsaved draft
 * can't launch the browser); a failed, cancelled, or timed-out sign-in is a
 * warning that points the user at the explicit retry path.
 */
function formatDatabricksAuthStatus(
  message: string,
): PersonaModelDiscoveryStatus | null {
  if (message.includes("Databricks sign-in is required")) {
    return {
      message: t("agents.modelStatus.databricksSignIn"),
      tone: "muted",
    };
  }

  if (
    message.includes("Databricks sign-in failed") ||
    message.includes("Databricks sign-in timed out")
  ) {
    return {
      message: t("agents.modelStatus.databricksIncomplete"),
      tone: "warning",
    };
  }

  return null;
}
