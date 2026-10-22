import {
  CreateProjectWorkItemDialog,
  type CreateProjectWorkItemDialogInput,
} from "./CreateProjectWorkItemDialog";
import { useT } from "@/shared/i18n";

export type CreateIssueDialogInput = CreateProjectWorkItemDialogInput;

export function CreateIssueDialog({
  isCreating,
  onCreate,
  onOpenChange,
  open,
  projectName,
}: {
  isCreating: boolean;
  onCreate: (input: CreateIssueDialogInput) => Promise<void>;
  onOpenChange: (open: boolean) => void;
  open: boolean;
  projectName: string;
}) {
  const t = useT();
  return (
    <CreateProjectWorkItemDialog
      bodyPlaceholder={t("projects.createTask.bodyPlaceholder")}
      description={t("projects.createTask.description", { name: projectName })}
      isCreating={isCreating}
      itemName="issue"
      onCreate={onCreate}
      onOpenChange={onOpenChange}
      open={open}
      title={t("projects.createTask.title")}
      titlePlaceholder={t("projects.createTask.titlePlaceholder")}
    />
  );
}
