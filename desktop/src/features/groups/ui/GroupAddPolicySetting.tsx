import { useMutation, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import {
  SettingsOptionGroup,
  SettingsOptionRow,
} from "@/features/settings/ui/SettingsOptionGroup";
import { useIdentityQuery } from "@/shared/api/hooks";
import { SegmentedControl } from "@/shared/ui/segmented-control";
import {
  type GroupAddPolicy,
  groupsErrorText,
  setGroupAddPolicy,
} from "../groupsApi";
import { groupAddPolicyQueryKey, useGroupAddPolicyQuery } from "../hooks";

const OPTIONS = [
  { value: "friends", label: "Friends" },
  { value: "everyone", label: "Everyone" },
] as const;

/**
 * Settings › Privacy: who can add me to groups. With Friends (the default),
 * anyone else sends an invitation instead.
 */
export function GroupAddPolicySetting() {
  const queryClient = useQueryClient();
  const account = useIdentityQuery().data?.pubkey?.toLowerCase() ?? "";
  const queryKey = groupAddPolicyQueryKey(account);
  const policyQuery = useGroupAddPolicyQuery();
  const save = useMutation({
    mutationFn: (next: GroupAddPolicy) => setGroupAddPolicy(next),
    onMutate: (next) => {
      const previous = queryClient.getQueryData<GroupAddPolicy>(queryKey);
      queryClient.setQueryData<GroupAddPolicy>(queryKey, next);
      return { previous };
    },
    onError: (error, _next, context) => {
      queryClient.setQueryData(queryKey, context?.previous);
      toast.error(groupsErrorText(error) || "Couldn't change this setting");
    },
  });

  return (
    <SettingsOptionGroup data-testid="settings-group-add-policy" title="Groups">
      <SettingsOptionRow>
        <p className="text-sm font-medium">Who can add me to groups</p>
        <SegmentedControl
          disabled={!policyQuery.isSuccess || save.isPending}
          legend="Who can add me to groups"
          onValueChange={(next) => {
            if (next !== policyQuery.data) save.mutate(next);
          }}
          optionTestIdPrefix="settings-group-add-policy"
          options={OPTIONS}
          size="compact"
          testId="settings-group-add-policy-control"
          value={policyQuery.data ?? "friends"}
        />
      </SettingsOptionRow>
      {policyQuery.isError ? (
        <SettingsOptionRow>
          <p className="text-sm text-muted-foreground">
            Couldn't load this setting.
          </p>
        </SettingsOptionRow>
      ) : null}
    </SettingsOptionGroup>
  );
}
