import * as React from "react";
import { toast } from "sonner";

import type { Repository } from "@/features/projects/hooks";
import {
  openProjectMergeRecoveryTerminal,
  openProjectRepositoryFolder,
} from "@/shared/api/projectGit";
import { useOpenProjectTerminal } from "./useOpenProjectTerminal";
import { useT } from "@/shared/i18n";

type MergeRecoveryInput = {
  expectedCommit: string;
  sourceBranch: string;
  sourceCloneUrl: string;
  targetBranch: string;
};

export function useProjectRepositoryOpenActions({
  activeBranch,
  hasLocalCheckout,
  localRepositoryPath,
  repository,
  reposDir,
}: {
  activeBranch: string | null;
  hasLocalCheckout: boolean;
  localRepositoryPath: string | null;
  repository: Repository | null | undefined;
  reposDir?: string | null;
}) {
  const t = useT();
  const openTerminal = useOpenProjectTerminal(reposDir);
  const handleOpenTerminal = React.useCallback(() => {
    if (!repository) return Promise.resolve();
    return openTerminal(repository, {
      branch: activeBranch,
      hasLocalCheckout,
    });
  }, [activeBranch, hasLocalCheckout, openTerminal, repository]);

  const handleOpenLocalRepository = React.useCallback(async () => {
    const cloneUrl = repository?.cloneUrls[0];
    if (!localRepositoryPath || !repository || !cloneUrl) {
      toast.error(t("projects.openFolder.failed"), {
        description: t("projects.openFolder.notFound"),
      });
      return;
    }
    try {
      await openProjectRepositoryFolder({
        cloneUrl,
        projectDtag: repository.dtag,
        reposDir,
      });
    } catch {
      toast.error(t("projects.openFolder.failed"), {
        description: t("projects.openFolder.openFailed"),
      });
    }
  }, [localRepositoryPath, repository, reposDir, t]);

  const handleOpenMergeRecoveryTerminal = React.useCallback(
    async (input: MergeRecoveryInput) => {
      const targetCloneUrl = repository?.cloneUrls[0];
      if (!repository || !targetCloneUrl) {
        throw new Error(t("projects.errors.noProjectSelected"));
      }
      return openProjectMergeRecoveryTerminal({
        ...input,
        projectDtag: repository.dtag,
        reposDir,
        targetCloneUrl,
      });
    },
    [repository, reposDir, t],
  );

  return {
    handleOpenLocalRepository,
    handleOpenMergeRecoveryTerminal,
    handleOpenTerminal,
  };
}
