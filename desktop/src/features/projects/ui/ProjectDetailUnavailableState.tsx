import { ArrowLeft, FolderGit2 } from "lucide-react";

import type { Project } from "@/features/projects/hooks";
import { Button } from "@/shared/ui/button";
import { UnavailableProjectRepositories } from "./UnavailableProjectRepositories";
import { useT } from "@/shared/i18n";

type ProjectDetailUnavailableStateProps =
  | {
      kind: "load-error";
      onBack: () => void;
      onRetry: () => void;
    }
  | {
      kind: "not-found";
      onBack: () => void;
    }
  | {
      kind: "repositories-unavailable";
      project: Project;
    };

export function ProjectDetailUnavailableState(
  props: ProjectDetailUnavailableStateProps,
) {
  const t = useT();
  if (props.kind === "load-error") {
    return (
      <div className="flex flex-1 flex-col items-center justify-center gap-3 px-4 py-16 text-center">
        <FolderGit2 className="h-10 w-10 text-muted-foreground/40" />
        <p className="text-sm text-red-400">
          {t("projects.detailUnavailable.loadFailed")}
        </p>
        <div className="flex items-center gap-2">
          <Button onClick={props.onRetry} size="sm" variant="outline">
            {t("common.retry")}
          </Button>
          <Button onClick={props.onBack} size="sm" variant="ghost">
            <ArrowLeft className="mr-1.5 h-4 w-4" />
            {t("projects.detailUnavailable.back")}
          </Button>
        </div>
      </div>
    );
  }

  if (props.kind === "not-found") {
    return (
      <div className="flex flex-1 flex-col items-center justify-center gap-3 px-4 py-16 text-center">
        <FolderGit2 className="h-10 w-10 text-muted-foreground/40" />
        <p className="text-sm text-muted-foreground">
          {t("projects.detailUnavailable.notFound")}
        </p>
        <Button onClick={props.onBack} size="sm" variant="outline">
          <ArrowLeft className="mr-1.5 h-4 w-4" />
          {t("projects.detailUnavailable.back")}
        </Button>
      </div>
    );
  }

  return (
    <div className="flex flex-1 flex-col items-center justify-center gap-3 px-4 py-16 text-center">
      <FolderGit2 className="h-10 w-10 text-muted-foreground/40" />
      <p className="text-sm font-medium text-foreground">
        {props.project.name}
      </p>
      <p className="text-sm text-muted-foreground">
        {t("projects.detailUnavailable.noRepositories")}
      </p>
      <UnavailableProjectRepositories project={props.project} />
    </div>
  );
}
