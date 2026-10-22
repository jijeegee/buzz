import { useAgentManagement } from "@/features/agents/useAgentManagement";
import { ProjectChannelRequestDialog } from "@/features/projects/ui/ProjectChannelRequestDialog";
import { AgentCardDialogs } from "./AgentCardViewerDialog";
import { AgentDialog } from "./AgentDialog";
import { useT } from "@/shared/i18n";

/** Global review surfaces opened by owned agents through the Kuzz harness. */
export function AgentManagementDialogs() {
  const t = useT();
  const management = useAgentManagement();

  return (
    <>
      {management.request?.action === "create" ? (
        <AgentDialog
          // A new agent's baseline is owner-only.
          draftReview={{ savedAccess: null }}
          definitionError={
            management.error ? new Error(management.error) : null
          }
          initialValues={management.createInitialValues}
          isDefinitionPending={management.isPending}
          mode="definition"
          onOpenChange={(open) => {
            if (!open) management.dismiss();
          }}
          onSubmitDefinition={management.submitCreate}
          runtimes={management.runtimes}
          runtimeCatalogStatus={management.runtimeCatalogStatus}
        />
      ) : null}
      {management.request?.action === "update" ? (
        <AgentDialog
          description=""
          draftReview={{ savedAccess: management.editSavedAccess }}
          error={management.editError ? new Error(management.editError) : null}
          initialValues={management.editInitialValues}
          isPending={management.isPending}
          mode="definition-edit"
          onOpenChange={(open) => {
            if (!open) management.dismiss();
          }}
          onSubmit={management.submitUpdate}
          open
          runtimes={management.runtimes}
          runtimeCatalogStatus={management.runtimeCatalogStatus}
          submitLabel={t("agents.dialog.saveChanges")}
          title={t("agents.dialog.editTitle")}
        />
      ) : null}
      <ProjectChannelRequestDialog />
      <AgentCardDialogs />
    </>
  );
}
