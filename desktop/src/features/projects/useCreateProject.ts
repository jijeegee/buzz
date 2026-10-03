import * as React from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import {
  channelsQueryKey,
  upsertCachedChannel,
} from "@/features/channels/hooks";
import { joinAgentsAfterCreate } from "@/features/agents/lib/joinAgentsAfterCreate";
import { useAttachDefaultAi } from "@/features/agents/useAttachDefaultAi";
import { useApplyTemplate } from "@/features/channel-templates/useApplyTemplate";
import { type Project, projectsQueryKey } from "@/features/projects/hooks";
import {
  createProject,
  type CreateProjectInput,
  type CreateProjectResult,
  type CreateProjectResumeState,
} from "@/features/projects/createProject";
import { addProjectToSidebar } from "@/features/projects/lib/projectSidebarMembership";
import {
  applyProjectHomeCanvas,
  PROJECT_HOME_TEMPLATE_ID,
} from "@/features/projects/lib/projectHomeTemplate";
import { markProjectDataAuthoritative } from "@/features/projects/projectSnapshot";
import type { Channel } from "@/shared/api/types";
import { getCachedRelayOrigin } from "@/shared/lib/mediaUrl";

export type { CreateProjectInput, CreateProjectResult };

/** Mutation that creates a project home and inserts it into the caches. */
export function useCreateProjectMutation() {
  const queryClient = useQueryClient();
  const { applyAgents, applyCanvas } = useApplyTemplate();
  const { attachDefaultAi } = useAttachDefaultAi();
  const resumeRef = React.useRef<CreateProjectResumeState>({
    channels: new Map(),
    projectIds: new Set(),
  });

  return useMutation({
    mutationFn: (input: CreateProjectInput) =>
      createProject(input, resumeRef.current),
    onSuccess: async ({ channel, project }, input) => {
      markProjectDataAuthoritative(project, "local-write");
      addProjectToSidebar(
        project.projectAddress,
        getCachedRelayOrigin(),
        project.owner,
      );
      queryClient.setQueryData<Project[]>(projectsQueryKey, (current = []) => [
        project,
        ...current.filter(
          (candidate) =>
            candidate.id !== project.id &&
            !(
              candidate.legacy &&
              candidate.owner === project.owner &&
              candidate.dtag === project.dtag
            ),
        ),
      ]);
      if (channel) {
        queryClient.setQueryData(
          channelsQueryKey,
          (current: Channel[] | undefined) =>
            upsertCachedChannel(current, channel),
        );
        void queryClient.invalidateQueries({
          queryKey: channelsQueryKey,
          refetchType: "none",
        });
        const useProjectHomeTemplate =
          input.templateId === undefined ||
          input.templateId === PROJECT_HOME_TEMPLATE_ID;
        if (useProjectHomeTemplate) {
          const applied = await applyProjectHomeCanvas({
            channelId: channel.id,
            project,
          });
          if (!applied) {
            toast.warning(
              "Project created, but its project-home canvas could not be added.",
            );
          }
        } else if (input.templateId) {
          await applyCanvas(input.templateId, channel.id, channel.name);
        }
        // Same order as the channel/forum/project-channel paths: default AI
        // first, then template personas (deduped against members). The
        // form-requested agents were already added inside `createProject`,
        // before this resolved. Fired after the canvas so navigation is not
        // held up by a spawn.
        void joinAgentsAfterCreate(
          { attachDefaultAi, applyAgents },
          {
            channelId: channel.id,
            templateId: useProjectHomeTemplate ? undefined : input.templateId,
            addDefaultAi: input.addDefaultAi,
          },
        );
      }
      void queryClient.invalidateQueries({ queryKey: projectsQueryKey });
    },
  });
}
