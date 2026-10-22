import { toast } from "sonner";

import type {
  AgentPersona,
  CreateManagedAgentResponse,
  CreatePersonaInput,
  UpdatePersonaInput,
} from "@/shared/api/types";
import { toUserFacingErrorDetail } from "@/shared/lib/userFacingError";

type SubmitProfilePersonaDialogOptions = {
  createManagedAgentForPersona: (
    persona: AgentPersona,
  ) => Promise<CreateManagedAgentResponse>;
  createPersona: (input: CreatePersonaInput) => Promise<AgentPersona>;
  input: CreatePersonaInput | UpdatePersonaInput;
  onDone: () => void;
  /** Called when the save landed on a new persona (this account's own copy
   *  of a built-in), so the panel can follow it. */
  onPersonaReplaced?: (persona: AgentPersona) => void;
  updatePersona: (input: UpdatePersonaInput) => Promise<AgentPersona>;
};

export async function submitProfilePersonaDialog({
  createManagedAgentForPersona,
  createPersona,
  input,
  onDone,
  onPersonaReplaced,
  updatePersona,
}: SubmitProfilePersonaDialogOptions) {
  try {
    if ("id" in input) {
      // A preset edit changes only the preset: agents made from it keep their
      // own copy of its settings.
      const persona = await updatePersona(input);
      // Editing a built-in saves this account's own copy under a new id.
      const replaced = persona.id !== input.id;
      toast.success(`Updated ${input.displayName}.`);
      if (replaced) onPersonaReplaced?.(persona);
    } else {
      const persona = await createPersona(input);
      try {
        const created = await createManagedAgentForPersona(persona);
        if (created.spawnError) {
          toast.error(
            `${persona.displayName} was created, but it did not start: ${toUserFacingErrorDetail(created.spawnError)}`,
          );
        } else {
          toast.success(`Created and started ${created.agent.name}.`);
        }
        if (created.profileSyncError) {
          toast.warning(
            `${created.agent.name} was created, but profile sync failed: ${toUserFacingErrorDetail(created.profileSyncError)}`,
          );
        }
      } catch (error) {
        toast.error(
          error instanceof Error
            ? `${persona.displayName} was created, but the agent instance could not be created: ${toUserFacingErrorDetail(error.message)}`
            : `${persona.displayName} was created, but the agent instance could not be created.`,
        );
      }
    }

    onDone();
  } catch (error) {
    toast.error(
      error instanceof Error
        ? toUserFacingErrorDetail(error.message)
        : "Failed to save agent.",
    );
  }
}
