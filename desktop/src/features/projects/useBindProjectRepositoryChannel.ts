import { useMutation, useQueryClient } from "@tanstack/react-query";

import {
  type Project,
  projectsQueryKey,
  type Repository,
} from "@/features/projects/hooks";
import { eventToRepository } from "@/features/projects/projectModels";
import { buildRepositoryChannelBindingTemplate } from "@/features/projects/projectRepositoryCreation";
import { inheritProjectDataProvenance } from "@/features/projects/projectSnapshot";
import { relayClient } from "@/shared/api/relayClient";
import { signRelayEvent } from "@/shared/api/tauri";
import { getIdentity } from "@/shared/api/tauriIdentity";
import { getCachedRelayOrigin } from "@/shared/lib/mediaUrl";
import { t } from "@/shared/i18n";

type BindProjectRepositoryChannelInput = {
  channelId: string;
  repository: Repository;
};

async function bindProjectRepositoryChannel({
  channelId,
  repository,
}: BindProjectRepositoryChannelInput): Promise<Repository> {
  const identity = await getIdentity();
  const template = buildRepositoryChannelBindingTemplate({
    channelId,
    ownerPubkey: identity.pubkey,
    repository,
  });
  const event = await signRelayEvent({
    ...template,
    createdAt: Math.max(
      Math.floor(Date.now() / 1_000),
      repository.createdAt + 1,
    ),
  });
  await relayClient.publishEvent(
    event,
    t("projects.useBindProjectRepositoryChannel.timedOut"),
    t("projects.useBindProjectRepositoryChannel.failed"),
  );

  const updated = eventToRepository(event, getCachedRelayOrigin());
  if (!updated) {
    throw new Error(t("projects.useBindProjectRepositoryChannel.readFailed"));
  }
  return updated;
}

export function useBindProjectRepositoryChannelMutation() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: bindProjectRepositoryChannel,
    onSuccess: (repository) => {
      queryClient.setQueryData<Project[]>(projectsQueryKey, (current = []) =>
        current.map((project) => {
          if (
            !project.repositories.some(
              (candidate) => candidate.repoAddress === repository.repoAddress,
            )
          ) {
            return project;
          }
          const updatedProject = {
            ...project,
            repositories: project.repositories.map((candidate) =>
              candidate.repoAddress === repository.repoAddress
                ? repository
                : candidate,
            ),
          };
          return inheritProjectDataProvenance(project, updatedProject);
        }),
      );
      void queryClient.invalidateQueries({ queryKey: projectsQueryKey });
    },
  });
}
